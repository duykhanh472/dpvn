//! Character set definitions and palette generation from `charsets.toml`.
//!
//! Character sets govern:
//! 1. Character validation during save (blocking error on unrecognized codepoints).
//! 2. Visual character palette buttons in the proofing UI.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

use crate::error::DomainError;

/// Embedded default charsets.toml.
pub const DEFAULT_CHARSETS_TOML: &str = include_str!("charsets.toml");

/// Raw charset definition in TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCharsetDef {
    /// Base letters (lowercase).
    #[serde(default)]
    pub letters: String,
    /// Permitted punctuation symbols.
    #[serde(default)]
    pub punctuation: String,
    /// Permitted typographic symbols.
    #[serde(default)]
    pub symbols: String,
    /// Name of another charset this definition inherits from.
    pub extends: Option<String>,
    /// Additional letters for extended charsets.
    #[serde(default)]
    pub letters_extra: String,
}

/// Fully resolved charset definition with fast lookup sets.
#[derive(Debug, Clone)]
pub struct Charset {
    /// Unique key (e.g. "vi", "vi-fr").
    pub name: String,
    /// Full set of permitted Unicode scalar values.
    pub allowed_chars: HashSet<char>,
    /// Selected palette characters for toolbar insert buttons.
    pub palette: Vec<char>,
}

impl Charset {
    /// Check whether a character is permitted under this charset.
    ///
    /// Standard whitespace characters (space, newline, tab) are always permitted.
    #[must_use]
    pub fn is_allowed(&self, c: char) -> bool {
        c == ' ' || c == '\n' || c == '\r' || c == '\t' || self.allowed_chars.contains(&c)
    }

    /// Validate an entire string slice, returning the first illegal codepoint if any.
    #[must_use]
    pub fn find_illegal_char(&self, text: &str) -> Option<char> {
        text.chars().find(|&c| !self.is_allowed(c))
    }
}

/// Collection of all active charset definitions.
#[derive(Debug, Clone)]
pub struct CharsetCatalog {
    charsets: HashMap<String, Charset>,
}

impl CharsetCatalog {
    /// Load and resolve the default embedded `charsets.toml`.
    ///
    /// # Panics
    /// Panics if the embedded `charsets.toml` is malformed (guaranteed valid at build time).
    #[must_use]
    pub fn default_embedded() -> Self {
        Self::parse(DEFAULT_CHARSETS_TOML).expect("embedded charsets.toml must be valid")
    }

    /// Parse a TOML string into a resolved [`CharsetCatalog`].
    ///
    /// # Errors
    /// Returns [`DomainError::Config`] if parsing fails or inheritance cannot be resolved.
    pub fn parse(toml_str: &str) -> Result<Self, DomainError> {
        let raw_map: HashMap<String, RawCharsetDef> = toml::from_str(toml_str)
            .map_err(|e| DomainError::Config(format!("failed to parse charsets.toml: {e}")))?;

        let mut resolved = HashMap::new();

        // 1. Resolve base charsets (those without `extends`)
        for (name, def) in &raw_map {
            if def.extends.is_none() {
                resolved.insert(name.clone(), build_charset(name, def, None));
            }
        }

        // 2. Resolve extended charsets
        for (name, def) in &raw_map {
            if let Some(ref parent_name) = def.extends {
                let parent = resolved.get(parent_name).ok_or_else(|| {
                    DomainError::Config(format!(
                        "charset '{name}' extends unknown parent '{parent_name}'"
                    ))
                })?;
                resolved.insert(name.clone(), build_charset(name, def, Some(parent)));
            }
        }

        Ok(Self { charsets: resolved })
    }

    /// Retrieve a charset by its configuration key (e.g. "vi").
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Charset> {
        self.charsets.get(name)
    }

    /// Retrieve the standard Vietnamese charset ("vi"), falling back to any available.
    #[must_use]
    pub fn default_vi(&self) -> &Charset {
        self.charsets.get("vi").unwrap_or_else(|| {
            self.charsets
                .values()
                .next()
                .expect("catalog must contain at least one charset")
        })
    }
}

/// Build a resolved [`Charset`] combining base letters, diacritics, punctuation, and symbols.
fn build_charset(name: &str, def: &RawCharsetDef, parent: Option<&Charset>) -> Charset {
    let mut allowed = HashSet::new();

    if let Some(p) = parent {
        allowed.extend(&p.allowed_chars);
    }

    // Add digits 0-9
    for d in '0'..='9' {
        allowed.insert(d);
    }

    // Process base letters and expand with Vietnamese tone marks
    let letters_to_expand = format!("{}{}", def.letters, def.letters_extra);
    for ch in letters_to_expand.chars() {
        allowed.insert(ch);
        // Include uppercase
        for up in ch.to_uppercase() {
            allowed.insert(up);
        }
    }

    // Expand all standard Vietnamese precomposed tone variants (NFC)
    expand_vietnamese_vowels(&mut allowed);

    // Add punctuation
    for p in def.punctuation.chars() {
        allowed.insert(p);
    }

    // Add typographic symbols
    for s in def.symbols.chars() {
        allowed.insert(s);
    }

    // Standard ASCII punctuation common in published books
    let common_ascii_punct = "-–—/\\@#$%^&+=<>";
    for c in common_ascii_punct.chars() {
        allowed.insert(c);
    }

    // Curate toolbar palette items (special characters that are hard to type)
    let mut palette = Vec::new();
    let priority_symbols = [
        'đ', 'Đ', '…', '—', '–', '«', '»', '“', '”', '‘', '’', '§', '¶', '†', '‡',
    ];
    for &sym in &priority_symbols {
        if allowed.contains(&sym) {
            palette.push(sym);
        }
    }

    // If French extended, add French-specific characters to palette
    if name.contains("fr") {
        let fr_extra = ['æ', 'Æ', 'œ', 'Œ', 'ç', 'Ç', 'ë', 'ï', 'ü', 'ÿ'];
        for &ch in &fr_extra {
            palette.push(ch);
            allowed.insert(ch);
        }
    }

    Charset {
        name: name.to_string(),
        allowed_chars: allowed,
        palette,
    }
}

/// Populate all NFC combinations of Vietnamese vowels with standard diacritics.
fn expand_vietnamese_vowels(set: &mut HashSet<char>) {
    // Standard Vietnamese vowels in lowercase
    let vowels = [
        // a-series
        'a', 'à', 'á', 'ả', 'ã', 'ạ', 'ă', 'ằ', 'ắ', 'ẳ', 'ẵ', 'ặ', 'â', 'ầ', 'ấ', 'ẩ', 'ẫ', 'ậ',
        // e-series
        'e', 'è', 'é', 'ẻ', 'ẽ', 'ẹ', 'ê', 'ề', 'ế', 'ể', 'ễ', 'ệ', // i-series
        'i', 'ì', 'í', 'ỉ', 'ĩ', 'ị', // o-series
        'o', 'ò', 'ó', 'ỏ', 'õ', 'ọ', 'ô', 'ồ', 'ố', 'ổ', 'ỗ', 'ộ', 'ơ', 'ờ', 'ớ', 'ở', 'ỡ', 'ợ',
        // u-series
        'u', 'ù', 'ú', 'ủ', 'ũ', 'ụ', 'ư', 'ừ', 'ứ', 'ử', 'ữ', 'ự', // y-series
        'y', 'ỳ', 'ý', 'ỷ', 'ỹ', 'ỵ',
    ];

    for &v in &vowels {
        // Ensure NFC
        for nfc_char in v.to_string().nfc() {
            set.insert(nfc_char);
            for up in nfc_char.to_uppercase() {
                set.insert(up);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_catalog_parses_vi() {
        let catalog = CharsetCatalog::default_embedded();
        let vi = catalog.get("vi").expect("vi charset must exist");

        // Common Vietnamese text
        assert!(vi.is_allowed('a'));
        assert!(vi.is_allowed('A'));
        assert!(vi.is_allowed('ế'));
        assert!(vi.is_allowed('Ế'));
        assert!(vi.is_allowed('đ'));
        assert!(vi.is_allowed('Đ'));
        assert!(vi.is_allowed('.'));
        assert!(vi.is_allowed('…'));
        assert!(vi.is_allowed('—'));

        // Palette must contain Vietnamese special chars
        assert!(vi.palette.contains(&'đ'));
        assert!(vi.palette.contains(&'…'));
        assert!(vi.palette.contains(&'—'));

        // Illegal characters (e.g. Cyrillic or emojis)
        assert!(!vi.is_allowed('Я'));
        assert!(!vi.is_allowed('🔥'));
        assert_eq!(vi.find_illegal_char("Tiếng Việt chuẩn"), None);
        assert_eq!(
            vi.find_illegal_char("Tiếng Việt có biểu tượng 🔥"),
            Some('🔥')
        );
        // 'j' is not in standard Vietnamese alphabet
        assert_eq!(vi.find_illegal_char("Tiếng Việt emoji"), Some('j'));
    }

    #[test]
    fn test_vi_fr_inherits_vi() {
        let catalog = CharsetCatalog::default_embedded();
        let vi_fr = catalog.get("vi-fr").expect("vi-fr charset must exist");

        // Inherits Vietnamese
        assert!(vi_fr.is_allowed('đ'));
        assert!(vi_fr.is_allowed('ế'));

        // Includes French extras
        assert!(vi_fr.is_allowed('æ'));
        assert!(vi_fr.is_allowed('œ'));
        assert!(vi_fr.is_allowed('ç'));
    }
}
