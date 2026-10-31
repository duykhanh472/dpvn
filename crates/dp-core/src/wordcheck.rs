//! Wordcheck issue types, character validation, and advisory checking.

use crate::charset::Charset;
use serde::{Deserialize, Serialize};

/// Category of issue discovered in proofed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// Character outside project charset or illegal control character.
    InvalidCharacter,
    /// Syllable violates Vietnamese phonotactic structure.
    InvalidSyllable,
    /// Word in project bad list.
    InProjectBadList,
    /// Word in site bad list.
    InSiteBadList,
    /// Accent placement conflicts with project tone style.
    ToneStyle,
}

impl IssueKind {
    /// Human-friendly description in Vietnamese.
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::InvalidCharacter => "Ký tự không hợp lệ",
            Self::InvalidSyllable => "Cấu trúc âm tiết lạ",
            Self::InProjectBadList => "Từ thuộc danh sách tránh dùng của dự án",
            Self::InSiteBadList => "Từ thuộc danh sách tránh dùng chung",
            Self::ToneStyle => "Phong cách bỏ dấu thanh không nhất quán",
        }
    }
}

/// An advisory issue flagging a word or character range in proofed text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordIssue {
    /// Problematic word or token substring.
    pub word: String,
    /// Start byte offset in original text (for CM6 decoration mapping).
    pub byte_start: usize,
    /// End byte offset in original text.
    pub byte_end: usize,
    /// Classification of issue.
    pub kind: IssueKind,
    /// Optional spelling correction or preferred replacement.
    pub suggestion: Option<String>,
}

/// Validate text against project charset and check for disallowed control characters.
///
/// Returns `Err(message)` on blocking violations (control chars or illegal codepoints).
///
/// # Errors
/// Returns error description string if text contains illegal characters.
pub fn validate_characters(text: &str, charset: &Charset) -> Result<(), String> {
    for (idx, ch) in text.char_indices() {
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            return Err(format!(
                "Ký tự điều khiển không hợp lệ (U+{:04X}) tại vị trí {idx}",
                ch as u32
            ));
        }
        if !charset.is_allowed(ch) {
            return Err(format!(
                "Ký tự '{ch}' (U+{:04X}) không thuộc bộ ký tự '{}' của dự án",
                ch as u32, charset.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::charset::CharsetCatalog;

    #[test]
    fn test_validate_characters_blocking() {
        let catalog = CharsetCatalog::default_embedded();
        let vi = catalog.get("vi").unwrap();

        assert!(validate_characters("Tiếng Việt chuẩn mực.\nĐoạn văn mới.", vi).is_ok());

        // Control char (e.g. Bell U+0007 or NULL U+0000)
        assert!(validate_characters("Có ký tự \u{0007} cấm", vi).is_err());

        // Unallowed character (e.g. Cyrillic)
        assert!(validate_characters("Ký tự Nga: Д", vi).is_err());
    }
}
