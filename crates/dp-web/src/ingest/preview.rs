//! Dry-run preview generation: page count, dimension stats, first lines of text, and warnings.

use serde::{Deserialize, Serialize};

use super::{image::ValidatedImage, pairing::PairedPage};

/// Preview entry for one page, shown in the dry-run confirmation UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagePreview {
    /// 1-based page number.
    pub page_number: i32,
    /// Image entry filename for display.
    pub image_entry: String,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Up to 20 lines of OCR text for preview.
    pub first_lines: Vec<String>,
    /// Non-fatal warnings for this specific page.
    pub warnings: Vec<String>,
}

/// Aggregate dry-run preview report returned to the user for acknowledgment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPreview {
    /// Total number of pages that would be committed.
    pub total_pages: usize,
    /// Number of pages with warnings (non-blocking).
    pub warning_pages: usize,
    /// Number of hard errors encountered during parsing (0 means commit is allowed).
    pub error_count: usize,
    /// Error messages from hard failures.
    pub errors: Vec<String>,
    /// Global warnings not attributable to a single page.
    pub global_warnings: Vec<String>,
    /// Per-page preview entries (first 50 shown in UI; full list committed).
    pub pages: Vec<PagePreview>,
}

impl IngestPreview {
    /// Returns `true` when there are no hard errors and a commit can proceed.
    #[must_use]
    pub fn can_commit(&self) -> bool {
        self.error_count == 0
    }

    /// Returns `true` when there are any warnings that need user acknowledgment.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.warning_pages > 0 || !self.global_warnings.is_empty()
    }
}

/// Build a preview from paired pages and their validated image metadata.
///
/// `validated_images` maps page_number → `ValidatedImage`.
#[must_use]
pub fn build_preview(
    paired: &[PairedPage],
    validated_images: &std::collections::HashMap<i32, ValidatedImage>,
    global_warnings: Vec<String>,
    errors: Vec<String>,
) -> IngestPreview {
    let error_count = errors.len();
    let mut page_previews = Vec::with_capacity(paired.len());
    let mut warning_pages = 0usize;

    for page in paired {
        let img = validated_images.get(&page.page_number);
        let (width, height) = img.map(|i| (i.width, i.height)).unwrap_or((0, 0));

        let first_lines: Vec<String> = page
            .ocr_text
            .lines()
            .take(20)
            .map(ToOwned::to_owned)
            .collect();

        if !page.warnings.is_empty() {
            warning_pages += 1;
        }

        page_previews.push(PagePreview {
            page_number: page.page_number,
            image_entry: page.image_entry.clone(),
            width,
            height,
            first_lines,
            warnings: page.warnings.clone(),
        });
    }

    IngestPreview {
        total_pages: paired.len(),
        warning_pages,
        error_count,
        errors,
        global_warnings,
        pages: page_previews,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_preview_can_commit_on_no_errors() {
        // No errors → can commit even with warnings present.
        let preview_with_warnings = IngestPreview {
            total_pages: 5,
            warning_pages: 2,
            error_count: 0,
            errors: vec![],
            global_warnings: vec![],
            pages: vec![],
        };
        assert!(
            preview_with_warnings.can_commit(),
            "should be committable with no hard errors"
        );
        assert!(
            preview_with_warnings.has_warnings(),
            "should report warnings when warning_pages > 0"
        );

        // No errors, no warnings → can commit cleanly.
        let preview_clean = IngestPreview {
            total_pages: 5,
            warning_pages: 0,
            error_count: 0,
            errors: vec![],
            global_warnings: vec![],
            pages: vec![],
        };
        assert!(preview_clean.can_commit());
        assert!(!preview_clean.has_warnings());
    }

    #[test]
    fn test_preview_cannot_commit_with_errors() {
        let preview = IngestPreview {
            total_pages: 0,
            warning_pages: 0,
            error_count: 1,
            errors: vec!["some error".into()],
            global_warnings: vec![],
            pages: vec![],
        };
        assert!(!preview.can_commit());
    }

    #[test]
    fn test_build_preview_basic() {
        let pages = vec![PairedPage {
            page_number: 1,
            image_entry: "0001.png".into(),
            image_ext: "png".into(),
            ocr_text: "Dòng một\nDòng hai\nDòng ba".into(),
            warnings: vec![],
        }];

        let mut images = HashMap::new();
        images.insert(
            1,
            ValidatedImage {
                image_type: super::super::image::ImageType::Png,
                width: 800,
                height: 1200,
            },
        );

        let preview = build_preview(&pages, &images, vec![], vec![]);
        assert_eq!(preview.total_pages, 1);
        assert_eq!(preview.pages[0].first_lines.len(), 3);
        assert_eq!(preview.pages[0].width, 800);
        assert!(preview.can_commit());
    }
}
