//! Comprehensive Vietnamese Wordcheck engine (§9).
//!
//! Evaluates text against three confidence signals in order (§9.2):
//! 1. Project word lists (`project_words`: `good` suppresses, `bad` flags).
//! 2. Site word lists (`site_words`: site-wide good/bad, project overrides).
//! 3. Phonotactic validity (rule-based validation of syllable onset, nucleus, coda, and tone).

pub mod distance;
pub mod phonotactic;
pub mod tokenizer;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::charset::Charset;
use crate::project::ToneStyle;
pub use distance::{find_best_suggestion, levenshtein_distance};
pub use phonotactic::{
    PHONOTACTIC_TABLES, PhonotacticError, PhonotacticTables, SyllableParts, Tone,
    check_syllable_structure, split_tone, validate_syllable,
};
pub use tokenizer::{SyllableToken, tokenize_syllables};

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

/// Environmental dictionary context for running wordcheck on page text.
#[derive(Debug, Clone)]
pub struct WordcheckContext<'a> {
    pub charset: &'a Charset,
    pub tone_style: ToneStyle,
    pub project_good: &'a HashSet<String>,
    pub project_bad: &'a HashSet<String>,
    pub site_good: &'a HashSet<String>,
    pub site_bad: &'a HashSet<String>,
}

/// Run full wordcheck evaluation on text slice and return all advisory issues (§9.2, §9.4).
pub fn check_page_text(text: &str, ctx: &WordcheckContext<'_>) -> Vec<WordIssue> {
    let mut issues = Vec::new();

    // 1. Scan for charset violations (blocking codepoints)
    // Track byte ranges already flagged so phonotactic pass skips them.
    let mut flagged_ranges: Vec<std::ops::Range<usize>> = Vec::new();

    for (idx, ch) in text.char_indices() {
        let is_illegal_control = ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t';
        if is_illegal_control || !ctx.charset.is_allowed(ch) {
            let end = idx + ch.len_utf8();
            flagged_ranges.push(idx..end);
            issues.push(WordIssue {
                word: ch.to_string(),
                byte_start: idx,
                byte_end: end,
                kind: IssueKind::InvalidCharacter,
                suggestion: None,
            });
        }
    }

    // Helper: check if a token range overlaps any already-flagged range
    let overlaps_flagged = |start: usize, end: usize| -> bool {
        flagged_ranges
            .iter()
            .any(|r| r.start < end && r.end > start)
    };

    // 2. Tokenize syllables and evaluate 3 signals
    let candidate_pool = ctx
        .project_good
        .iter()
        .chain(ctx.site_good.iter())
        .map(|s| s.as_str());

    for tok in tokenize_syllables(text) {
        // Skip tokens that overlap with a charset-violation range (already flagged above)
        if overlaps_flagged(tok.byte_start, tok.byte_end) {
            continue;
        }

        let has_upper = tok.word.chars().any(|c| c.is_uppercase());
        let tok_lower_cow = if has_upper {
            std::borrow::Cow::Owned(tok.word.to_lowercase())
        } else {
            std::borrow::Cow::Borrowed(tok.word)
        };
        let tok_lower: &str = &tok_lower_cow;

        // Signal 1: Project word lists (Project wins)
        if !ctx.project_bad.is_empty() && ctx.project_bad.contains(tok_lower) {
            let sug = find_best_suggestion(tok.word, candidate_pool.clone(), 2);
            issues.push(WordIssue {
                word: tok.word.to_string(),
                byte_start: tok.byte_start,
                byte_end: tok.byte_end,
                kind: IssueKind::InProjectBadList,
                suggestion: sug,
            });
            continue;
        }

        if !ctx.project_good.is_empty() && ctx.project_good.contains(tok_lower) {
            // Explicitly allowed for this project, suppress warnings
            continue;
        }

        // Signal 2: Site word lists
        if !ctx.site_bad.is_empty() && ctx.site_bad.contains(tok_lower) {
            let sug = find_best_suggestion(tok.word, candidate_pool.clone(), 2);
            issues.push(WordIssue {
                word: tok.word.to_string(),
                byte_start: tok.byte_start,
                byte_end: tok.byte_end,
                kind: IssueKind::InSiteBadList,
                suggestion: sug,
            });
            continue;
        }

        if !ctx.site_good.is_empty() && ctx.site_good.contains(tok_lower) {
            // Known valid site word - verify tone style if configured
            if ctx.tone_style != ToneStyle::Either {
                if let Ok((_, Some(detected))) =
                    check_syllable_structure(tok.word, ctx.tone_style, &PHONOTACTIC_TABLES)
                {
                    if detected != ctx.tone_style {
                        issues.push(WordIssue {
                            word: tok.word.to_string(),
                            byte_start: tok.byte_start,
                            byte_end: tok.byte_end,
                            kind: IssueKind::ToneStyle,
                            suggestion: None,
                        });
                    }
                }
            }
            continue;
        }

        // Signal 3: Phonotactic validity
        match check_syllable_structure(tok.word, ctx.tone_style, &PHONOTACTIC_TABLES) {
            Ok(_) => {
                // Syllable is phonotactically valid!
            }
            Err(PhonotacticError::ToneStyleMismatch { .. }) => {
                issues.push(WordIssue {
                    word: tok.word.to_string(),
                    byte_start: tok.byte_start,
                    byte_end: tok.byte_end,
                    kind: IssueKind::ToneStyle,
                    suggestion: None,
                });
            }
            Err(_) => {
                let sug = find_best_suggestion(tok.word, candidate_pool.clone(), 2);
                issues.push(WordIssue {
                    word: tok.word.to_string(),
                    byte_start: tok.byte_start,
                    byte_end: tok.byte_end,
                    kind: IssueKind::InvalidSyllable,
                    suggestion: sug,
                });
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::charset::CharsetCatalog;

    #[test]
    fn test_three_signal_resolution() {
        let catalog = CharsetCatalog::default_embedded();
        let vi = catalog.get("vi").unwrap();

        let mut project_good = HashSet::new();
        project_good.insert("shakespeare".to_string()); // Foreign name permitted by project

        let mut project_bad = HashSet::new();
        project_bad.insert("trẫm".to_string()); // Word discouraged for this project

        let mut site_good = HashSet::new();
        site_good.insert("người".to_string());

        let site_bad = HashSet::new();

        let ctx = WordcheckContext {
            charset: vi,
            tone_style: ToneStyle::Either,
            project_good: &project_good,
            project_bad: &project_bad,
            site_good: &site_good,
            site_bad: &site_bad,
        };

        let text = "Shakespeare nói với trẫm rằng người này viết sai xxxx.";
        let issues = check_page_text(text, &ctx);

        // "Shakespeare" is in project_good -> NOT flagged
        assert!(!issues.iter().any(|i| i.word == "Shakespeare"));

        // "trẫm" is in project_bad -> FLAGGED InProjectBadList
        assert!(
            issues
                .iter()
                .any(|i| i.word == "trẫm" && i.kind == IssueKind::InProjectBadList)
        );

        // "người" is in site_good -> NOT flagged
        assert!(!issues.iter().any(|i| i.word == "người"));

        // "xxxx" is phonotactically invalid -> FLAGGED InvalidSyllable
        assert!(
            issues
                .iter()
                .any(|i| i.word == "xxxx" && i.kind == IssueKind::InvalidSyllable)
        );
    }
}
