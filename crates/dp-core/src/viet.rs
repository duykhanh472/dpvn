//! Vietnamese text handling, normalization, and character utilities.
//!
//! Vietnamese text must be normalized to Unicode Normalization Form C (NFC),
//! have Byte Order Marks (BOM) stripped, and standardize line endings to LF (`\n`).

use std::borrow::Cow;
use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};

/// Normalizes Vietnamese input text according to specification:
/// 1. Strips UTF-8 Byte Order Mark (`\u{FEFF}`) if present.
/// 2. Converts Windows (`\r\n`) and legacy Mac (`\r`) line endings to UNIX LF (`\n`).
/// 3. Normalizes all characters to Unicode Normalization Form C (NFC).
#[must_use]
pub fn normalize(raw: &str) -> String {
    let s = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    s.nfc().collect()
}

/// Normalizes Vietnamese input text conditionally using `Cow`.
///
/// Avoids allocation when the text has no BOM, has clean LF line endings,
/// and is already in NFC form.
#[must_use]
pub fn normalize_cow(raw: &str) -> Cow<'_, str> {
    if !raw.starts_with('\u{FEFF}')
        && !raw.contains('\r')
        && is_nfc_quick(raw.chars()) == IsNormalized::Yes
    {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(normalize(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_bom_stripped() {
        let input = "\u{FEFF}Tiếng Việt";
        let output = normalize(input);
        assert_eq!(output, "Tiếng Việt");
        assert!(!output.starts_with('\u{FEFF}'));
    }

    #[test]
    fn test_line_endings_normalized() {
        let input = "Dòng 1\r\nDòng 2\rDòng 3\nDòng 4";
        let output = normalize(input);
        assert_eq!(output, "Dòng 1\nDòng 2\nDòng 3\nDòng 4");
    }

    #[test]
    fn test_nfd_to_nfc() {
        // 'ế': decomposed as 'ê' (U+00EA) + combining acute (U+0301)
        let decomposed = "\u{00EA}\u{0301}";
        assert_eq!(decomposed.chars().count(), 2);

        let composed = normalize(decomposed);
        assert_eq!(composed.chars().count(), 1);
        assert_eq!(composed, "ế");
        assert_eq!(composed, "\u{1EBF}");
    }

    #[test]
    fn test_cow_borrowed_when_already_normalized() {
        let clean = "Tiếng Việt sạch sẽ không có BOM";
        match normalize_cow(clean) {
            Cow::Borrowed(s) => assert_eq!(s, clean),
            Cow::Owned(_) => panic!("Expected clean string to be borrowed without allocation"),
        }
    }

    #[test]
    fn test_cow_owned_when_modification_needed() {
        let dirty = "\u{FEFF}Tiếng Việt\r\n";
        match normalize_cow(dirty) {
            Cow::Owned(s) => assert_eq!(s, "Tiếng Việt\n"),
            Cow::Borrowed(_) => panic!("Expected dirty string to be owned after normalization"),
        }
    }

    proptest! {
        // Normalization must be idempotent: normalize(normalize(s)) == normalize(s)
        #[test]
        fn prop_nfc_idempotent(s in "\\PC{0,500}") {
            let once = normalize(&s);
            let twice = normalize(&once);
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn prop_cow_matches_owned(s in "\\PC{0,500}") {
            let owned = normalize(&s);
            let cow = normalize_cow(&s);
            prop_assert_eq!(owned, cow.into_owned());
        }
    }
}
