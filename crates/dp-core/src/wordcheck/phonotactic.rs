//! Vietnamese phonotactic syllable validation and tone analysis (§9.3).
//!
//! Evaluates syllable structure according to standard Vietnamese phonology:
//! `onset (phụ âm đầu) + nucleus (âm chính) + coda (âm cuối) + tone (thanh điệu)`
//!
//! Also checks:
//! - Stop codas (`p`, `t`, `c`, `ch`) can only carry Sắc (acute) or Nặng (dot) tones.
//! - Short vowels `ă` and `â` require a coda.
//! - Orthographic constraints: `k`/`gh`/`ngh` before front vowels (`i`/`e`/`ê`), `c`/`g`/`ng` before back vowels.
//! - Tone style alignment (`old` vs `new`).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::project::ToneStyle;

/// The six Vietnamese phonemic tones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Không dấu (level tone)
    Ngang,
    /// Dấu huyền (grave: \ )
    Huyen,
    /// Dấu sắc (acute: / )
    Sac,
    /// Dấu hỏi (hook above: ? )
    Hoi,
    /// Dấu ngã (tilde: ~ )
    Nga,
    /// Dấu nặng (dot below: . )
    Nang,
}

/// Structured components of a parsed Vietnamese syllable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyllableParts {
    pub onset: String,
    pub nucleus: String,
    pub coda: String,
    pub tone: Tone,
    pub detected_tone_style: Option<ToneStyle>,
}

/// Reason for phonotactic rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonotacticError {
    Empty,
    MultipleTones,
    InvalidOnset(String),
    InvalidNucleus(String),
    InvalidCoda(String),
    StopCodaInvalidTone(String, Tone),
    ShortVowelWithoutCoda(String),
    FrontVowelSpellingMismatch(String),
    ToneStyleMismatch {
        expected: ToneStyle,
        detected: ToneStyle,
    },
}

#[derive(Debug, Deserialize)]
struct RawPhonotacticsToml {
    onsets: RawList,
    codas: RawList,
    stop_codas: RawList,
    nuclei: RawNuclei,
}

#[derive(Debug, Deserialize)]
struct RawList {
    items: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawNuclei {
    monophthongs: Vec<String>,
    polyphthongs: Vec<String>,
}

/// Compiled phonotactic lookup sets for high-performance validation (< 5µs).
pub struct PhonotacticTables {
    pub onsets: HashSet<String>,
    pub sorted_onsets: Vec<String>,
    pub codas: HashSet<String>,
    pub sorted_codas: Vec<String>,
    pub stop_codas: HashSet<String>,
    pub nuclei: HashSet<String>,
}

pub const EMBEDDED_PHONOTACTICS_TOML: &str = include_str!("phonotactics.toml");

pub static PHONOTACTIC_TABLES: LazyLock<PhonotacticTables> = LazyLock::new(|| {
    let raw: RawPhonotacticsToml = toml::from_str(EMBEDDED_PHONOTACTICS_TOML)
        .expect("embedded phonotactics.toml must be valid");

    let mut onsets_set = HashSet::new();
    let mut sorted_onsets = raw.onsets.items;
    // Sort onsets by descending length for greedy maximal-onset decomposition
    sorted_onsets.sort_by_key(|b| std::cmp::Reverse(b.len()));
    for o in &sorted_onsets {
        onsets_set.insert(o.clone());
    }

    let mut codas_set = HashSet::new();
    let mut sorted_codas = raw.codas.items;
    sorted_codas.sort_by_key(|b| std::cmp::Reverse(b.len()));
    for c in &sorted_codas {
        codas_set.insert(c.clone());
    }

    let stop_codas: HashSet<String> = raw.stop_codas.items.into_iter().collect();

    let mut nuclei = HashSet::new();
    for m in raw.nuclei.monophthongs {
        nuclei.insert(m);
    }
    for p in raw.nuclei.polyphthongs {
        nuclei.insert(p);
    }

    PhonotacticTables {
        onsets: onsets_set,
        sorted_onsets,
        codas: codas_set,
        sorted_codas,
        stop_codas,
        nuclei,
    }
});

/// Map a Vietnamese accented character to its base un-toned vowel and Tone.
fn strip_char_tone(c: char) -> (char, Tone) {
    match c {
        // a-series
        'à' | 'À' => ('a', Tone::Huyen),
        'á' | 'Á' => ('a', Tone::Sac),
        'ả' | 'Ả' => ('a', Tone::Hoi),
        'ã' | 'Ã' => ('a', Tone::Nga),
        'ạ' | 'Ạ' => ('a', Tone::Nang),

        // ă-series
        'ằ' | 'Ằ' => ('ă', Tone::Huyen),
        'ắ' | 'Ắ' => ('ă', Tone::Sac),
        'ẳ' | 'Ẳ' => ('ă', Tone::Hoi),
        'ẵ' | 'Ẵ' => ('ă', Tone::Nga),
        'ặ' | 'Ặ' => ('ă', Tone::Nang),

        // â-series
        'ầ' | 'Ầ' => ('â', Tone::Huyen),
        'ấ' | 'Ấ' => ('â', Tone::Sac),
        'ẩ' | 'Ẩ' => ('â', Tone::Hoi),
        'ẫ' | 'Ẫ' => ('â', Tone::Nga),
        'ậ' | 'Ậ' => ('â', Tone::Nang),

        // e-series
        'è' | 'È' => ('e', Tone::Huyen),
        'é' | 'É' => ('e', Tone::Sac),
        'ẻ' | 'Ẻ' => ('e', Tone::Hoi),
        'ẽ' | 'Ẽ' => ('e', Tone::Nga),
        'ẹ' | 'Ẹ' => ('e', Tone::Nang),

        // ê-series
        'ề' | 'Ề' => ('ê', Tone::Huyen),
        'ế' | 'Ế' => ('ê', Tone::Sac),
        'ể' | 'Ể' => ('ê', Tone::Hoi),
        'ễ' | 'Ễ' => ('ê', Tone::Nga),
        'ệ' | 'Ệ' => ('ê', Tone::Nang),

        // i-series
        'ì' | 'Ì' => ('i', Tone::Huyen),
        'í' | 'Í' => ('i', Tone::Sac),
        'ỉ' | 'Ỉ' => ('i', Tone::Hoi),
        'ĩ' | 'Ĩ' => ('i', Tone::Nga),
        'ị' | 'Ị' => ('i', Tone::Nang),

        // o-series
        'ò' | 'Ò' => ('o', Tone::Huyen),
        'ó' | 'Ó' => ('o', Tone::Sac),
        'ỏ' | 'Ỏ' => ('o', Tone::Hoi),
        'õ' | 'Õ' => ('o', Tone::Nga),
        'ọ' | 'Ọ' => ('o', Tone::Nang),

        // ô-series
        'ồ' | 'Ồ' => ('ô', Tone::Huyen),
        'ố' | 'Ố' => ('ô', Tone::Sac),
        'ổ' | 'Ổ' => ('ô', Tone::Hoi),
        'ỗ' | 'Ỗ' => ('ô', Tone::Nga),
        'ộ' | 'Ộ' => ('ô', Tone::Nang),

        // ơ-series
        'ờ' | 'Ờ' => ('ơ', Tone::Huyen),
        'ớ' | 'Ớ' => ('ơ', Tone::Sac),
        'ở' | 'Ở' => ('ơ', Tone::Hoi),
        'ỡ' | 'Ỡ' => ('ơ', Tone::Nga),
        'ợ' | 'Ợ' => ('ơ', Tone::Nang),

        // u-series
        'ù' | 'Ù' => ('u', Tone::Huyen),
        'ú' | 'Ú' => ('u', Tone::Sac),
        'ủ' | 'Ủ' => ('u', Tone::Hoi),
        'ũ' | 'Ũ' => ('u', Tone::Nga),
        'ụ' | 'Ụ' => ('u', Tone::Nang),

        // ư-series
        'ừ' | 'Ừ' => ('ư', Tone::Huyen),
        'ứ' | 'Ứ' => ('ư', Tone::Sac),
        'ử' | 'Ử' => ('ư', Tone::Hoi),
        'ữ' | 'Ữ' => ('ư', Tone::Nga),
        'ự' | 'Ự' => ('ư', Tone::Nang),

        // y-series
        'ỳ' | 'Ỳ' => ('y', Tone::Huyen),
        'ý' | 'Ý' => ('y', Tone::Sac),
        'ỷ' | 'Ỷ' => ('y', Tone::Hoi),
        'ỹ' | 'Ỹ' => ('y', Tone::Nga),
        'ỵ' | 'Ỵ' => ('y', Tone::Nang),

        // non-ASCII consonants
        'Đ' => ('đ', Tone::Ngang),

        other => {
            let first = other.to_lowercase().next().unwrap_or(other);
            (first, Tone::Ngang)
        }
    }
}

/// Separate the tone from a syllable, returning `(tone, base_syllable, detected_tone_style)`.
///
/// Returns `None` if the syllable contains multiple contradictory tone marks (e.g. `*tòá`).
pub fn split_tone(raw_word: &str) -> Option<(Tone, String, Option<ToneStyle>)> {
    if raw_word.is_empty() {
        return None;
    }

    let mut base = String::with_capacity(raw_word.len());
    let mut detected_tone = Tone::Ngang;
    let mut tone_char_index: Option<usize> = None;
    let mut vowel_indices = [0usize; 4];
    let mut vowel_count = 0;

    for (char_idx, ch) in raw_word.chars().enumerate() {
        let (base_ch, tone) = strip_char_tone(ch);
        base.push(base_ch);

        let is_vowel = matches!(
            base_ch,
            'a' | 'ă' | 'â' | 'e' | 'ê' | 'i' | 'o' | 'ô' | 'ơ' | 'u' | 'ư' | 'y'
        );
        if is_vowel && vowel_count < 4 {
            vowel_indices[vowel_count] = char_idx;
            vowel_count += 1;
        }

        if tone != Tone::Ngang {
            if detected_tone != Tone::Ngang {
                // Multiple tones detected on different vowels!
                return None;
            }
            detected_tone = tone;
            tone_char_index = Some(char_idx);
        }
    }

    // Determine tone style for open diphthongs (oa, oe, uy)
    let mut tone_style = None;
    if let Some(t_idx) = tone_char_index {
        if vowel_count == 2 {
            let v1_idx = vowel_indices[0];
            let v2_idx = vowel_indices[1];
            let v1 = base.chars().nth(v1_idx).unwrap_or(' ');
            let v2 = base.chars().nth(v2_idx).unwrap_or(' ');

            // Applies to pairs like oa, oe, uy without codas
            let is_style_pair = (v1 == 'o' && (v2 == 'a' || v2 == 'e'))
                || (v1 == 'u' && (v2 == 'y' || v2 == 'a' || v2 == 'e'));

            if is_style_pair {
                if t_idx == v1_idx {
                    tone_style = Some(ToneStyle::Old);
                } else if t_idx == v2_idx {
                    tone_style = Some(ToneStyle::New);
                }
            }
        }
    }

    Some((detected_tone, base, tone_style))
}

/// Decompose a normalized un-toned base syllable into `(onset, nucleus, coda)`.
pub fn decompose_syllable<'a>(
    base: &'a str,
    tables: &'a PhonotacticTables,
) -> Option<(&'a str, &'a str, &'a str)> {
    if base.is_empty() {
        return None;
    }

    // 1. Identify Onset
    let mut onset = "";
    let mut rest = base;

    // Special case for 'qu'
    if let Some(stripped) = base.strip_prefix("qu") {
        onset = "qu";
        rest = stripped;
    }
    // Special case for 'gi'
    else if let Some(after_gi) = base.strip_prefix("gi") {
        // If followed by another vowel (e.g. 'già', 'gió', 'giếng'), 'gi' is onset
        if after_gi.starts_with(['a', 'ă', 'â', 'e', 'ê', 'o', 'ô', 'ơ', 'u', 'ư']) {
            onset = "gi";
            rest = after_gi;
        } else if base == "gi" {
            // 'gì' / 'gi' alone -> onset 'gi', nucleus 'i' (or onset 'g', nucleus 'i')
            onset = "gi";
            rest = "i";
        } else {
            onset = "g";
            rest = &base[1..];
        }
    } else {
        // Greedy longest match from valid onsets
        for cand in &tables.sorted_onsets {
            if !cand.is_empty() && base.starts_with(cand) {
                onset = cand.as_str();
                rest = &base[cand.len()..];
                break;
            }
        }
    }

    // 2. Identify Coda (greedy match from end)
    let mut coda = "";
    let mut nucleus = rest;

    for cand in &tables.sorted_codas {
        if !cand.is_empty() && rest.ends_with(cand) {
            let potential_nucleus = &rest[..rest.len() - cand.len()];
            // Ensure potential nucleus is non-empty and a valid nucleus
            if tables.nuclei.contains(potential_nucleus) {
                coda = cand.as_str();
                nucleus = potential_nucleus;
                break;
            }
        }
    }

    // If no coda matched with a valid nucleus, check if whole remainder is a valid nucleus
    if coda.is_empty() && !tables.nuclei.contains(nucleus) {
        // Try fallback decomposition
        return None;
    }

    Some((onset, nucleus, coda))
}

/// Lightweight phonotactic check returning `(Tone, Option<ToneStyle>)` without cloning components.
pub fn check_syllable_structure(
    word: &str,
    target_tone_style: ToneStyle,
    tables: &PhonotacticTables,
) -> Result<(Tone, Option<ToneStyle>), PhonotacticError> {
    let Some((tone, base, detected_tone_style)) = split_tone(word) else {
        return Err(PhonotacticError::MultipleTones);
    };

    let Some((onset, nucleus, coda)) = decompose_syllable(&base, tables) else {
        return Err(PhonotacticError::InvalidNucleus(word.to_string()));
    };

    // Rule 1: Onset check
    if !tables.onsets.contains(onset) {
        return Err(PhonotacticError::InvalidOnset(onset.to_string()));
    }

    // Rule 2: Nucleus check
    if !tables.nuclei.contains(nucleus) {
        return Err(PhonotacticError::InvalidNucleus(nucleus.to_string()));
    }

    // Rule 3: Coda check
    if !tables.codas.contains(coda) {
        return Err(PhonotacticError::InvalidCoda(coda.to_string()));
    }

    // Rule 4: Checked syllables (stop codas p, t, c, ch only admit Sắc and Nặng)
    if tables.stop_codas.contains(coda) && tone != Tone::Sac && tone != Tone::Nang {
        return Err(PhonotacticError::StopCodaInvalidTone(
            coda.to_string(),
            tone,
        ));
    }

    // Rule 5: Short vowels ă and â cannot be in open syllables
    if (nucleus == "ă" || nucleus == "â") && coda.is_empty() {
        return Err(PhonotacticError::ShortVowelWithoutCoda(nucleus.to_string()));
    }

    // Rule 6: Orthographic rules for front vowels (i, e, ê, y)
    let is_front_nucleus = nucleus.starts_with('i')
        || nucleus.starts_with('e')
        || nucleus.starts_with('ê')
        || nucleus.starts_with('y');

    if is_front_nucleus {
        if onset == "c" || onset == "g" || onset == "ng" {
            return Err(PhonotacticError::FrontVowelSpellingMismatch(format!(
                "'{onset}' không đi với nguyên âm hàng trước '{nucleus}' (cần dùng k/gh/ngh)"
            )));
        }
    } else if !nucleus.is_empty() && (onset == "k" || onset == "gh" || onset == "ngh") {
        return Err(PhonotacticError::FrontVowelSpellingMismatch(format!(
            "'{onset}' chỉ đi với nguyên âm hàng trước i/e/ê/y, không đi với '{nucleus}'"
        )));
    }

    // Rule 7: Tone style check
    if target_tone_style != ToneStyle::Either {
        if let Some(detected) = detected_tone_style {
            if detected != target_tone_style {
                return Err(PhonotacticError::ToneStyleMismatch {
                    expected: target_tone_style,
                    detected,
                });
            }
        }
    }

    Ok((tone, detected_tone_style))
}

/// Validate a Vietnamese syllable phonotactically and check tone style (§9.3, §8.4).
///
/// Returns `Ok(parts)` if valid, or `Err(PhonotacticError)` with the specific violation.
pub fn validate_syllable(
    word: &str,
    target_tone_style: ToneStyle,
    tables: &PhonotacticTables,
) -> Result<SyllableParts, PhonotacticError> {
    let (tone, detected_tone_style) = check_syllable_structure(word, target_tone_style, tables)?;
    let Some((_, base, _)) = split_tone(word) else {
        return Err(PhonotacticError::MultipleTones);
    };
    let Some((onset, nucleus, coda)) = decompose_syllable(&base, tables) else {
        return Err(PhonotacticError::InvalidNucleus(word.to_string()));
    };

    Ok(SyllableParts {
        onset: onset.to_string(),
        nucleus: nucleus.to_string(),
        coda: coda.to_string(),
        tone,
        detected_tone_style,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_vietnamese_syllables() {
        let tables = &PHONOTACTIC_TABLES;

        let valids = [
            "nghiêng", "khoảng", "truyền", "thuyết", "quốc", "gia", "giúp", "giữ", "toàn", "bác",
            "bạc", "việt", "nam", "sách", "đẹp", "hòa", "hoà", "thuý", "thúy",
        ];

        for word in valids {
            let res = validate_syllable(word, ToneStyle::Either, tables);
            assert!(
                res.is_ok(),
                "Word '{word}' should be valid, got: {:?}",
                res.err()
            );
        }
    }

    #[test]
    fn test_stop_coda_rejects_invalid_tones() {
        let tables = &PHONOTACTIC_TABLES;

        // "bác" and "bạc" are valid (sắc, nặng)
        assert!(validate_syllable("bác", ToneStyle::Either, tables).is_ok());
        assert!(validate_syllable("bạc", ToneStyle::Either, tables).is_ok());

        // Stop codas cannot have ngang, huyền, hỏi, ngã
        assert!(matches!(
            validate_syllable("bac", ToneStyle::Either, tables),
            Err(PhonotacticError::StopCodaInvalidTone(_, Tone::Ngang))
        ));
        assert!(matches!(
            validate_syllable("bàc", ToneStyle::Either, tables),
            Err(PhonotacticError::StopCodaInvalidTone(_, Tone::Huyen))
        ));
        assert!(matches!(
            validate_syllable("bảc", ToneStyle::Either, tables),
            Err(PhonotacticError::StopCodaInvalidTone(_, Tone::Hoi))
        ));
        assert!(matches!(
            validate_syllable("bãc", ToneStyle::Either, tables),
            Err(PhonotacticError::StopCodaInvalidTone(_, Tone::Nga))
        ));
    }

    #[test]
    fn test_front_vowel_spelling_rules() {
        let tables = &PHONOTACTIC_TABLES;

        // Valid front: "khi", "ghế", "nghề", "kim"
        assert!(validate_syllable("khi", ToneStyle::Either, tables).is_ok());
        assert!(validate_syllable("ghế", ToneStyle::Either, tables).is_ok());
        assert!(validate_syllable("nghề", ToneStyle::Either, tables).is_ok());
        assert!(validate_syllable("kim", ToneStyle::Either, tables).is_ok());

        // Invalid: *cim, *gế, *ngề
        assert!(validate_syllable("cim", ToneStyle::Either, tables).is_err());
        assert!(validate_syllable("gế", ToneStyle::Either, tables).is_err());
        assert!(validate_syllable("ngề", ToneStyle::Either, tables).is_err());

        // Invalid: *ka (must be "ca"), *ghà (must be "gà")
        assert!(validate_syllable("ka", ToneStyle::Either, tables).is_err());
        assert!(validate_syllable("ghà", ToneStyle::Either, tables).is_err());
    }

    #[test]
    fn test_tone_style_detection() {
        let tables = &PHONOTACTIC_TABLES;

        // "hòa" is Old style, "hoà" is New style
        let res_old = validate_syllable("hòa", ToneStyle::Old, tables);
        assert!(res_old.is_ok());
        assert_eq!(res_old.unwrap().detected_tone_style, Some(ToneStyle::Old));

        let res_mismatch = validate_syllable("hòa", ToneStyle::New, tables);
        assert!(matches!(
            res_mismatch,
            Err(PhonotacticError::ToneStyleMismatch {
                expected: ToneStyle::New,
                detected: ToneStyle::Old,
            })
        ));

        let res_new = validate_syllable("hoà", ToneStyle::New, tables);
        assert!(res_new.is_ok());
        assert_eq!(res_new.unwrap().detected_tone_style, Some(ToneStyle::New));
    }
}
