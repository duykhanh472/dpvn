//! Ingest pipeline for importing zip archives into draft projects.
//!
//! ## Stages
//! 1. `inspect` — entry count, uncompressed size, path traversal checks
//! 2. `pair`    — Format A or Format B page pairing
//! 3. `validate_images` — magic bytes, dimension checks
//! 4. `dry_run` — returns an [`IngestPreview`] for user acknowledgment
//! 5. `commit`  — atomic file copy + transactional DB insert
//! 6. `derive`  — spawn display derivative generation (non-fatal)

pub mod error;
pub mod image;
pub mod pairing;
pub mod preview;
pub mod zip;

use std::{collections::HashMap, path::PathBuf};

use dp_db::queries::{
    pages::{NewPageInput, insert_pages_transactional},
    projects::delete_project_pages,
};
use sqlx::MySqlPool;
use tokio::fs;
use tracing::{info, instrument, warn};

use self::{
    error::IngestError,
    image::ValidatedImage,
    pairing::PairedPage,
    preview::{IngestPreview, build_preview},
};

/// Maximum upload size in bytes pulled from config. Hard limit enforced at the router level.
pub const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB

// ───────────────────────────────────────────────────────────────────────────────
// Public entry points
// ───────────────────────────────────────────────────────────────────────────────

/// Run the first half of the pipeline: parse the zip and return a dry-run preview.
///
/// Call this before showing the preview page to the manager. Nothing is written to
/// disk or the database at this stage.
///
/// # Errors
/// Returns [`IngestError`] on any hard failure (corrupt zip, invalid image, etc.).
#[instrument(skip(zip_bytes))]
pub async fn dry_run(zip_bytes: &[u8]) -> Result<IngestPreview, IngestError> {
    // 1. Inspect the zip (entry count, uncompressed size, path traversal)
    let entries = tokio::task::spawn_blocking({
        let data = zip_bytes.to_vec();
        move || zip::inspect_zip(&data)
    })
    .await
    .map_err(|e| IngestError::NotAZip(format!("spawn_blocking panicked: {e}")))?;

    let entries = entries?;

    // 2. Collect file (non-directory) entry names
    let file_names: Vec<&str> = entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.name.as_str())
        .collect();

    // 3. Pair pages (Format A or Format B)
    let pair_result = {
        let data = zip_bytes.to_vec();
        let file_names_owned: Vec<String> = file_names.iter().map(|s| s.to_string()).collect();
        tokio::task::spawn_blocking(move || {
            let file_name_refs: Vec<&str> = file_names_owned.iter().map(|s| s.as_str()).collect();
            pairing::pair_entries(&file_name_refs, &|name| {
                zip::read_entry(&data, name).map_err(|e| std::io::Error::other(e.to_string()))
            })
        })
        .await
        .map_err(|e| IngestError::NotAZip(format!("spawn_blocking panicked: {e}")))?
    }?;

    // 4. Validate images in parallel (bounded by semaphore inside validate_image)
    let (validated_images, image_errors) = validate_all_images(zip_bytes, &pair_result.pages).await;

    // 5. Build the preview report
    let preview = build_preview(
        &pair_result.pages,
        &validated_images,
        pair_result.global_warnings,
        image_errors,
    );

    info!(
        total = preview.total_pages,
        errors = preview.error_count,
        warnings = preview.warning_pages,
        "dry_run complete"
    );

    Ok(preview)
}

/// Commit the ingest after a successful dry run and manager acknowledgment.
///
/// Steps:
/// 1. If `replace` is true, delete existing pages from the database.
/// 2. Re-run parsing to get paired pages + validated images (idempotent).
/// 3. Copy original images to `data_dir/images/{project_id}/` via write-then-rename.
/// 4. Insert pages + page_rounds in a single database transaction.
/// 5. Spawn non-fatal display derivative generation.
///
/// On any hard failure the database transaction is rolled back and no partial
/// data can exist (no orphaned rows). If image files were already copied before
/// a DB failure, `dp prune` will clean them up within 24 hours.
///
/// # Errors
/// Returns [`IngestError`] if zip parsing, file I/O, or database insertion fails.
#[instrument(skip(pool, zip_bytes), fields(project_id))]
pub async fn commit(
    pool: &MySqlPool,
    zip_bytes: &[u8],
    project_id: u64,
    data_dir: &str,
    replace: bool,
) -> Result<Vec<dp_db::queries::pages::InsertedPage>, IngestError> {
    if replace {
        let deleted = delete_project_pages(pool, project_id).await?;
        info!(project_id, deleted, "replaced existing pages");
    }

    // Re-parse (cheap since zip is already in memory)
    let entries = tokio::task::spawn_blocking({
        let data = zip_bytes.to_vec();
        move || zip::inspect_zip(&data)
    })
    .await
    .map_err(|e| IngestError::NotAZip(format!("spawn_blocking panicked: {e}")))?;

    let entries = entries?;

    let file_names_owned: Vec<String> = entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.name.clone())
        .collect();

    let pair_result = {
        let data = zip_bytes.to_vec();
        let file_names_owned2 = file_names_owned.clone();
        tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = file_names_owned2.iter().map(|s| s.as_str()).collect();
            pairing::pair_entries(&refs, &|name| {
                zip::read_entry(&data, name).map_err(|e| std::io::Error::other(e.to_string()))
            })
        })
        .await
        .map_err(|e| IngestError::NotAZip(format!("spawn_blocking panicked: {e}")))?
    }?;

    let (validated_images, image_errors) = validate_all_images(zip_bytes, &pair_result.pages).await;

    if !image_errors.is_empty() {
        return Err(IngestError::InvalidImage {
            file: "multiple".to_owned(),
            reason: image_errors.join("; "),
        });
    }

    // Create the images destination directory
    let images_dir = PathBuf::from(data_dir)
        .join("images")
        .join(project_id.to_string());
    let tmp_dir = PathBuf::from(data_dir)
        .join("tmp")
        .join(format!("{project_id}-commit"));

    fs::create_dir_all(&images_dir)
        .await
        .map_err(|e| IngestError::Io {
            path: images_dir.clone(),
            source: e,
        })?;
    fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| IngestError::Io {
            path: tmp_dir.clone(),
            source: e,
        })?;

    // Build DB page inputs and copy image bytes to tmp
    let mut new_pages: Vec<NewPageInput> = Vec::with_capacity(pair_result.pages.len());
    let mut image_bytes_map: HashMap<i32, Vec<u8>> = HashMap::new();

    for page in &pair_result.pages {
        let img =
            validated_images
                .get(&page.page_number)
                .ok_or_else(|| IngestError::InvalidImage {
                    file: page.image_entry.clone(),
                    reason: "không tìm thấy thông tin ảnh đã xác thực".to_owned(),
                })?;

        let raw_bytes =
            zip::read_entry(zip_bytes, &page.image_entry).map_err(|e| IngestError::Io {
                path: PathBuf::from(&page.image_entry),
                source: std::io::Error::other(e.to_string()),
            })?;

        image_bytes_map.insert(page.page_number, raw_bytes);

        new_pages.push(NewPageInput {
            page_number: page.page_number,
            extension: page.image_ext.clone(),
            image_width: img.width,
            image_height: img.height,
            ocr_text: if page.ocr_text.is_empty() {
                None
            } else {
                Some(page.ocr_text.clone())
            },
        });
    }

    // Insert pages + initial page_rounds transactionally
    let inserted = insert_pages_transactional(pool, project_id, &new_pages).await?;

    // Now copy image files using page_id from inserted rows (atomic rename)
    let mut derivative_tasks = Vec::new();

    for ins_page in &inserted {
        let src_bytes = image_bytes_map
            .remove(&ins_page.page_number)
            .unwrap_or_default();

        // image_path is "images/{project_id}/{page_id}.{ext}"
        let dest = PathBuf::from(data_dir).join(&ins_page.image_path);
        let parent = dest.parent().unwrap_or(&dest).to_path_buf();
        fs::create_dir_all(&parent)
            .await
            .map_err(|e| IngestError::Io {
                path: parent.clone(),
                source: e,
            })?;

        // Write to tmp first, then rename (atomic on same filesystem)
        let tmp_path = tmp_dir.join(format!("{}.tmp", ins_page.page_id));
        fs::write(&tmp_path, &src_bytes)
            .await
            .map_err(|e| IngestError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;
        fs::rename(&tmp_path, &dest)
            .await
            .map_err(|e| IngestError::Io {
                path: dest.clone(),
                source: e,
            })?;

        // Queue display derivative generation (non-fatal)
        let display_dir = PathBuf::from(data_dir)
            .join("display")
            .join(project_id.to_string());
        let display_path = display_dir.join(format!("{}.jpg", ins_page.page_id));
        let src_bytes_for_derive = src_bytes.clone();
        derivative_tasks.push(tokio::spawn(async move {
            // Ensure display directory exists
            if let Err(e) = fs::create_dir_all(&display_dir).await {
                warn!(path = %display_dir.display(), error = %e, "could not create display dir");
                return;
            }

            match image::generate_display_derivative(src_bytes_for_derive).await {
                Ok(jpeg_bytes) => {
                    if let Err(e) = fs::write(&display_path, jpeg_bytes).await {
                        warn!(path = %display_path.display(), error = %e, "failed to write display derivative");
                    }
                }
                Err(e) => {
                    warn!(path = %display_path.display(), error = %e, "display derivative generation failed (non-fatal)");
                }
            }
        }));
    }

    // Clean up tmp dir
    if let Err(e) = fs::remove_dir_all(&tmp_dir).await {
        warn!(path = %tmp_dir.display(), error = %e, "failed to clean up tmp directory");
    }

    info!(project_id, pages = inserted.len(), "ingest commit complete");

    // Drive derivative tasks in background — do not await them here.
    tokio::spawn(async move {
        for task in derivative_tasks {
            let _ = task.await;
        }
    });

    Ok(inserted)
}

// ───────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ───────────────────────────────────────────────────────────────────────────────

/// Validate images for all paired pages, collecting successes and error messages.
///
/// Returns `(validated_map, error_messages)`. The caller decides whether errors
/// are hard failures (commit) or soft notices (dry_run).
async fn validate_all_images(
    zip_bytes: &[u8],
    pages: &[PairedPage],
) -> (HashMap<i32, ValidatedImage>, Vec<String>) {
    let mut validated = HashMap::new();
    let mut errors = Vec::new();

    for page in pages {
        let entry_name = page.image_entry.clone();
        let page_num = page.page_number;

        let bytes = match zip::read_entry(zip_bytes, &entry_name) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("trang {page_num} ('{entry_name}'): {e}"));
                continue;
            }
        };

        let entry_name2 = entry_name.clone();
        let _permit = match image::IMAGE_SEMAPHORE.acquire().await {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("semaphore lỗi: {e}"));
                break;
            }
        };

        match tokio::task::spawn_blocking(move || image::validate_image(&entry_name2, &bytes)).await
        {
            Ok(Ok(v)) => {
                validated.insert(page_num, v);
            }
            Ok(Err(e)) => {
                errors.push(format!("trang {page_num}: {e}"));
            }
            Err(e) => {
                errors.push(format!("trang {page_num}: spawn_blocking thất bại: {e}"));
            }
        }
    }

    (validated, errors)
}
