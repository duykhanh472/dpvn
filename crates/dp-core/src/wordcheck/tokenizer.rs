//! Syllable tokenizer for Vietnamese text (§9.1).
//!
//! Extracts space- and punctuation-separated word tokens with exact byte offsets
//! using the Unicode-aware regular expression ported from upstream DP.

use regex::Regex;
use std::sync::LazyLock;

/// Upstream-compatible Vietnamese word/syllable regex (§9.1).
/// Matches letters, marks, and interior apostrophes (e.g. O'Neill, d'or).
pub static WORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?u)[\p{L}\p{M}]+(?:['’][\p{L}\p{M}]+)*").expect("valid regex"));

/// A token extracted from source text with byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyllableToken<'a> {
    /// Token text slice.
    pub word: &'a str,
    /// UTF-8 start byte offset.
    pub byte_start: usize,
    /// UTF-8 end byte offset.
    pub byte_end: usize,
}

/// Tokenize an input string into Vietnamese syllables/words.
pub fn tokenize_syllables(text: &str) -> impl Iterator<Item = SyllableToken<'_>> {
    WORD_RE.find_iter(text).map(|m| SyllableToken {
        word: m.as_str(),
        byte_start: m.start(),
        byte_end: m.end(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_tokenize_basic_vietnamese() {
        let text = "Trăm năm, trong cõi người ta...";
        let tokens: Vec<_> = tokenize_syllables(text).collect();

        assert_eq!(tokens.len(), 6);
        assert_eq!(tokens[0].word, "Trăm");
        assert_eq!(tokens[0].byte_start, 0);
        assert_eq!(tokens[1].word, "năm");
        assert_eq!(tokens[2].word, "trong");
        assert_eq!(tokens[3].word, "cõi");
        assert_eq!(tokens[4].word, "người");
        assert_eq!(tokens[5].word, "ta");
    }

    #[test]
    fn test_tokenize_apostrophe_words() {
        let text = "L'amour và D'Artagnan";
        let tokens: Vec<_> = tokenize_syllables(text).collect();

        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].word, "L'amour");
        assert_eq!(tokens[1].word, "và");
        assert_eq!(tokens[2].word, "D'Artagnan");
    }

    proptest! {
        #[test]
        fn prop_token_offsets_always_valid_substrings(s in "\\PC*") {
            for tok in tokenize_syllables(&s) {
                // Must be within text bounds
                prop_assert!(tok.byte_start <= tok.byte_end);
                prop_assert!(tok.byte_end <= s.len());
                // Substring must match exactly
                prop_assert_eq!(&s[tok.byte_start..tok.byte_end], tok.word);
            }
        }
    }
}
