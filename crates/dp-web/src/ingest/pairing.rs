//! Page pairing logic for Format A (paired files) and Format B (image + marker-based text file).
//!
//! Format A: each page is `NNNN.png` + `NNNN.txt` (or `.jpg`/`.jpeg`).
//! Format B: images in `images/` directory + one `text.txt` file with `<<<PAGE:NNNN>>>` markers.

use std::collections::{BTreeMap, HashMap};

use dp_core::viet::normalize;

/// A recognized image extension (normalized lowercase without leading dot).
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg"];
/// A recognized text extension (normalized lowercase without leading dot).
const TEXT_EXT: &str = "txt";
/// Marker format for Format B text splits.
const PAGE_MARKER_PREFIX: &str = "<<<PAGE:";
const PAGE_MARKER_SUFFIX: &str = ">>>";

/// Maximum allowed text per page in characters (graphemes are not counted here; `.chars()` used).
pub const MAX_PAGE_TEXT_CHARS: usize = 200_000;

/// A fully paired, validated page ready for the dry-run preview and commit step.
#[derive(Debug, Clone)]
pub struct PairedPage {
    /// 1-based page number.
    pub page_number: i32,
    /// Zip entry name of the original image.
    pub image_entry: String,
    /// Canonical extension for the image ('png', 'jpg').
    pub image_ext: String,
    /// Normalized OCR text for this page (may be empty).
    pub ocr_text: String,
    /// Non-fatal warnings for display in the dry-run preview.
    pub warnings: Vec<String>,
}

/// Result of pairing including any global (non-page-specific) warnings.
#[derive(Debug, Clone)]
pub struct PairResult {
    /// Pages in ascending page_number order.
    pub pages: Vec<PairedPage>,
    /// Global warnings not attributable to a single page.
    pub global_warnings: Vec<String>,
}

/// Detect which format the upload uses and dispatch to the appropriate parser.
///
/// # Errors
/// Returns [`super::error::IngestError::NoPagesFound`] when no pages can be constructed.
pub fn pair_entries(
    file_entries: &[&str],
    read_bytes: &impl Fn(&str) -> Result<Vec<u8>, std::io::Error>,
) -> Result<PairResult, super::error::IngestError> {
    // Detect Format B: presence of `text.txt` at the root or anywhere.
    let has_text_txt = file_entries.iter().any(|n| {
        let n_lower = n.to_lowercase();
        n_lower == "text.txt" || n_lower.ends_with("/text.txt")
    });

    // Detect Format B images directory
    let has_images_dir = file_entries
        .iter()
        .any(|n| n.to_lowercase().starts_with("images/"));

    if has_text_txt && has_images_dir {
        pair_format_b(file_entries, read_bytes)
    } else {
        pair_format_a(file_entries, read_bytes)
    }
}

/// Parse Format A: `NNNN.png` + `NNNN.txt` pairs.
fn pair_format_a(
    entries: &[&str],
    read_bytes: &impl Fn(&str) -> Result<Vec<u8>, std::io::Error>,
) -> Result<PairResult, super::error::IngestError> {
    let mut image_map: BTreeMap<i32, (String, String)> = BTreeMap::new(); // num → (entry, ext)
    let mut text_map: BTreeMap<i32, String> = BTreeMap::new(); // num → text_entry

    for &name in entries {
        let stem = stem_of(name);

        if let Some(ext) = extension_of(name) {
            if IMAGE_EXTS.contains(&ext.as_str()) {
                if let Ok(num) = stem.parse::<i32>() {
                    let canonical = if ext == "jpeg" { "jpg" } else { ext.as_str() }.to_owned();
                    image_map.insert(num, (name.to_owned(), canonical));
                }
            } else if ext == TEXT_EXT {
                if let Ok(num) = stem.parse::<i32>() {
                    // Read and normalize text
                    let bytes = read_bytes(name).map_err(|e| super::error::IngestError::Io {
                        path: std::path::PathBuf::from(name),
                        source: e,
                    })?;
                    let raw = decode_utf8(name, &bytes)?;
                    text_map.insert(num, normalize(&raw));
                }
            }
        }
    }

    build_paired_pages_from_maps(image_map, text_map)
}

/// Parse Format B: `images/NNNN.png` + single `text.txt` with `<<<PAGE:NNNN>>>` markers.
fn pair_format_b(
    entries: &[&str],
    read_bytes: &impl Fn(&str) -> Result<Vec<u8>, std::io::Error>,
) -> Result<PairResult, super::error::IngestError> {
    // 1. Collect image entries from `images/` prefix.
    let mut image_map: BTreeMap<i32, (String, String)> = BTreeMap::new();
    let mut text_entry_name = "";

    for &name in entries {
        let lower = name.to_lowercase();
        if lower == "text.txt" || lower.ends_with("/text.txt") {
            text_entry_name = name;
            continue;
        }
        let basename = std::path::Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(name);
        let stem = stem_of(basename);

        if let Some(ext) = extension_of(basename) {
            if IMAGE_EXTS.contains(&ext.as_str()) {
                if let Ok(num) = stem.parse::<i32>() {
                    let canonical = if ext == "jpeg" { "jpg" } else { ext.as_str() }.to_owned();
                    image_map.insert(num, (name.to_owned(), canonical));
                }
            }
        }
    }

    // 2. Parse text file into per-page segments.
    let text_map = if text_entry_name.is_empty() {
        HashMap::new()
    } else {
        let bytes = read_bytes(text_entry_name).map_err(|e| super::error::IngestError::Io {
            path: std::path::PathBuf::from(text_entry_name),
            source: e,
        })?;
        let raw = decode_utf8(text_entry_name, &bytes)?;
        let normalized = normalize(&raw);
        parse_page_markers(&normalized)
    };

    // Convert text_map to BTreeMap
    let text_btree: BTreeMap<i32, String> = text_map.into_iter().collect();
    build_paired_pages_from_maps(image_map, text_btree)
}

/// Parse `<<<PAGE:NNNN>>>` markers from a normalized text blob.
fn parse_page_markers(text: &str) -> HashMap<i32, String> {
    let mut result = HashMap::new();
    let mut current_num: Option<i32> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(inner) = trimmed
            .strip_prefix(PAGE_MARKER_PREFIX)
            .and_then(|s| s.strip_suffix(PAGE_MARKER_SUFFIX))
        {
            // Save previous page
            if let Some(num) = current_num {
                let page_text = current_lines.join("\n");
                result.insert(num, page_text.trim_matches('\n').to_owned());
                current_lines.clear();
            }
            current_num = inner.parse().ok();
        } else if current_num.is_some() {
            current_lines.push(line);
        }
    }

    // Save last page
    if let Some(num) = current_num {
        let page_text = current_lines.join("\n");
        result.insert(num, page_text.trim_matches('\n').to_owned());
    }

    result
}

/// Assemble `PairedPage` list from image and text maps.
fn build_paired_pages_from_maps(
    image_map: BTreeMap<i32, (String, String)>,
    text_map: BTreeMap<i32, String>,
) -> Result<PairResult, super::error::IngestError> {
    if image_map.is_empty() {
        return Err(super::error::IngestError::NoPagesFound);
    }

    let mut pages = Vec::with_capacity(image_map.len());
    let mut global_warnings = Vec::new();

    let mut prev_num = 0i32;

    for (num, (entry, ext)) in &image_map {
        let mut warnings = Vec::new();

        // Check for page number gaps
        if prev_num > 0 && *num != prev_num + 1 {
            warnings.push(format!(
                "khoảng trống số trang: thiếu trang {} đến {}",
                prev_num + 1,
                num - 1
            ));
        }

        let text = match text_map.get(num) {
            Some(t) => {
                if t.is_empty() {
                    warnings.push(format!("trang {num}: văn bản trống"));
                } else if t.chars().count() > MAX_PAGE_TEXT_CHARS {
                    warnings.push(format!(
                        "trang {num}: văn bản rất dài ({} ký tự, khuyến nghị ≤ {})",
                        t.chars().count(),
                        MAX_PAGE_TEXT_CHARS
                    ));
                }
                t.clone()
            }
            None => {
                warnings.push(format!("trang {num}: không có tập tin văn bản tương ứng"));
                String::new()
            }
        };

        pages.push(PairedPage {
            page_number: *num,
            image_entry: entry.clone(),
            image_ext: ext.clone(),
            ocr_text: text,
            warnings,
        });

        prev_num = *num;
    }

    // Warn about orphaned text entries (text file but no matching image)
    for num in text_map.keys() {
        if !image_map.contains_key(num) {
            global_warnings.push(format!(
                "văn bản trang {num} không có tập tin ảnh tương ứng"
            ));
        }
    }

    Ok(PairResult {
        pages,
        global_warnings,
    })
}

/// Return the lowercase extension of a filename, without the leading dot.
fn extension_of(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
}

/// Return the file stem (name without extension).
fn stem_of(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_owned()
}

/// Decode raw bytes as UTF-8, mapping failures to [`IngestError::InvalidUtf8`].
fn decode_utf8(filename: &str, data: &[u8]) -> Result<String, super::error::IngestError> {
    std::str::from_utf8(data)
        .map(ToOwned::to_owned)
        .map_err(|e| super::error::IngestError::InvalidUtf8 {
            file: filename.to_owned(),
            reason: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_page_markers_basic() {
        let text = "<<<PAGE:0001>>>\nNội dung trang một\n<<<PAGE:0002>>>\nNội dung trang hai\n";
        let map = parse_page_markers(text);
        assert_eq!(map.get(&1).map(|s| s.as_str()), Some("Nội dung trang một"));
        assert_eq!(map.get(&2).map(|s| s.as_str()), Some("Nội dung trang hai"));
    }

    #[test]
    fn test_parse_page_markers_empty_page() {
        let text = "<<<PAGE:0001>>>\n<<<PAGE:0002>>>\nSome text";
        let map = parse_page_markers(text);
        // Page 1 is empty between the two markers
        let p1 = map.get(&1).map(|s| s.trim()).unwrap_or("");
        assert_eq!(p1, "");
    }

    #[test]
    fn test_extension_of() {
        assert_eq!(extension_of("file.PNG"), Some("png".into()));
        assert_eq!(extension_of("file.jpeg"), Some("jpeg".into()));
        assert_eq!(extension_of("noext"), None);
    }

    #[test]
    fn test_stem_of() {
        assert_eq!(stem_of("0001.png"), "0001");
        assert_eq!(stem_of("page-1.txt"), "page-1");
    }
}
