//! Database queries for project pages and their initial round states.

use sqlx::MySqlPool;

use crate::DbError;
use crate::models::Page;

/// DTO for a new page to be inserted during ingest.
#[derive(Debug, Clone)]
pub struct NewPageInput {
    /// 1-based page number.
    pub page_number: i32,
    /// Image file extension without leading dot (e.g. "png", "jpg").
    pub extension: String,
    /// Image width in pixels.
    pub image_width: u32,
    /// Image height in pixels.
    pub image_height: u32,
    /// Baseline normalized OCR text.
    pub ocr_text: Option<String>,
}

/// Result of an inserted page containing assigned IDs and final storage path.
#[derive(Debug, Clone)]
pub struct InsertedPage {
    /// Generated primary key ID.
    pub page_id: u64,
    /// Page sequence number.
    pub page_number: i32,
    /// Relative image storage path: `images/{project_id}/{page_id}.{extension}`.
    pub image_path: String,
}

/// Atomically insert all pages for a project and create initial 'P1' page_rounds entries.
///
/// If any page fails to insert or the transaction aborts, no changes are committed to the database.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if any insertion fails.
pub async fn insert_pages_transactional(
    pool: &MySqlPool,
    project_id: u64,
    pages: &[NewPageInput],
) -> Result<Vec<InsertedPage>, DbError> {
    let mut tx = pool.begin().await?;
    let mut results = Vec::with_capacity(pages.len());

    for page in pages {
        // 1. Insert page row with placeholder path
        let res = sqlx::query(
            "INSERT INTO pages (project_id, page_number, image_path, image_width, image_height, ocr_text)
             VALUES (?, ?, '', ?, ?, ?)",
        )
        .bind(project_id)
        .bind(page.page_number)
        .bind(page.image_width)
        .bind(page.image_height)
        .bind(&page.ocr_text)
        .execute(&mut *tx)
        .await?;

        let page_id = res.last_insert_id();
        let image_path = format!("images/{}/{}.{}", project_id, page_id, page.extension);

        // 2. Update image_path with the actual allocated page_id
        sqlx::query("UPDATE pages SET image_path = ? WHERE id = ?")
            .bind(&image_path)
            .bind(page_id)
            .execute(&mut *tx)
            .await?;

        // 3. Create initial round entry in 'P1' with state 'available'
        // OCR text seeds the initial round text.
        sqlx::query(
            "INSERT INTO page_rounds (page_id, round, state, text)
             VALUES (?, 'P1', 'available', ?)",
        )
        .bind(page_id)
        .bind(&page.ocr_text)
        .execute(&mut *tx)
        .await?;

        results.push(InsertedPage {
            page_id,
            page_number: page.page_number,
            image_path,
        });
    }

    tx.commit().await?;
    Ok(results)
}

/// Retrieve all pages belonging to a project, ordered by `page_number`.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_pages_by_project_id(
    pool: &MySqlPool,
    project_id: u64,
) -> Result<Vec<Page>, DbError> {
    let rows = sqlx::query_as::<_, Page>(
        "SELECT id, project_id, page_number, image_path, image_width, image_height, ocr_text
         FROM pages
         WHERE project_id = ?
         ORDER BY page_number ASC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Find a single page by its ID.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_page_by_id(pool: &MySqlPool, page_id: u64) -> Result<Option<Page>, DbError> {
    let row = sqlx::query_as::<_, Page>(
        "SELECT id, project_id, page_number, image_path, image_width, image_height, ocr_text
         FROM pages
         WHERE id = ?",
    )
    .bind(page_id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Find a page by project ID and page number.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_page_by_project_and_number(
    pool: &MySqlPool,
    project_id: u64,
    page_number: i32,
) -> Result<Option<Page>, DbError> {
    let row = sqlx::query_as::<_, Page>(
        "SELECT id, project_id, page_number, image_path, image_width, image_height, ocr_text
         FROM pages
         WHERE project_id = ? AND page_number = ?",
    )
    .bind(project_id)
    .bind(page_number)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Count total pages for a project.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn count_pages_by_project_id(pool: &MySqlPool, project_id: u64) -> Result<i64, DbError> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pages WHERE project_id = ?")
        .bind(project_id)
        .fetch_one(pool)
        .await?;

    Ok(count)
}
