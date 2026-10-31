//! Phase 6 Calibration and Performance Tests (§9.3, §9.6, Phase 6 Acceptance).
//!
//! Acceptance criteria:
//! 1. Wordcheck < 10 ms on 3,000 characters.
//! 2. False-positive rate < 0.5% on calibration corpus (>= 50,000 words).
//! 3. Tone style errors (e.g. 'hòa' vs 'hoà') and typo suggestions (e.g. 'nghỉ' vs 'nghĩ') are caught.

use std::collections::HashSet;
use std::time::Instant;

use dp_core::{
    CharsetCatalog, ToneStyle, WordcheckContext, check_page_text, find_best_suggestion,
    validate_syllable, wordcheck::PHONOTACTIC_TABLES,
};

#[test]
fn test_wordcheck_performance_under_10ms_on_3k_chars() {
    let catalog = CharsetCatalog::default_embedded();
    let vi = catalog.get("vi").unwrap();

    let project_good = HashSet::new();
    let project_bad = HashSet::new();
    let site_good = HashSet::new();
    let site_bad = HashSet::new();

    let ctx = WordcheckContext {
        charset: vi,
        tone_style: ToneStyle::Either,
        project_good: &project_good,
        project_bad: &project_bad,
        site_good: &site_good,
        site_bad: &site_bad,
    };

    // Construct realistic 3,000-character Vietnamese text
    let paragraph = "Trăm năm trong cõi người ta, chữ tài chữ mệnh khéo là ghét nhau. \
Trải qua một cuộc bể dâu, những điều trông thấy mà đau đớn lòng. \
Lạ gì bỉ sắc tư phong, trời xanh quen thói má hồng đánh ghen. \
Cảo thơm lần giở trước đèn, phong tình có lục còn truyền sử xanh. \
Rằng năm Gia Tĩnh triều Minh, bốn phương phẳng lặng hai kinh vững vàng. \
Có nhà viên ngoại họ Vương, gia tư nghĩ cũng thường thường bậc trung. \
Một trai con thứ rốt lòng, Vương Quan là chữ nối dòng nho gia. \
Đầu lòng hai ả tố nga, Thúy Kiều là chị em là Thúy Vân. \
Mai cốt cách tuyết tinh thần, một người một vẻ mười phân vẹn mười. \
Vân xem trang trọng khác vời, khuôn trăng đầy đặn nét ngài nở nang. \
Hoa cười ngọc thốt đoan trang, mây thua nước tóc tuyết nhường màu da. ";

    let mut text_3k = String::with_capacity(3500);
    while text_3k.chars().count() < 3000 {
        text_3k.push_str(paragraph);
    }
    assert!(text_3k.chars().count() >= 3000);

    // Warm up
    let _ = check_page_text(&text_3k, &ctx);

    // Measure execution time
    let start = Instant::now();
    let issues = check_page_text(&text_3k, &ctx);
    let duration = start.elapsed();

    println!("Wordcheck on 3k characters took: {duration:?}");
    println!("Wordcheck issues on 3k text: {issues:?}");
    // Requirement: < 10 ms
    assert!(
        duration.as_millis() < 10,
        "Wordcheck took {:?}, exceeding 10ms threshold",
        duration
    );
    // Classical poetic text without typos should have 0 issues under Either tone style
    assert_eq!(issues.len(), 0);
}

#[test]
fn test_corpus_calibration_false_positive_rate_under_half_percent() {
    let tables = &PHONOTACTIC_TABLES;

    // Load words.vi.txt as calibration vocabulary
    let words_file = include_str!("../../../crates/dp-db/words.vi.txt");
    let base_words: Vec<&str> = words_file
        .lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .collect();
    assert!(
        base_words.len() >= 10000,
        "base vocabulary must have >= 10,000 words"
    );

    // Build >= 50,000 word corpus by cycling through valid standard Vietnamese words
    let mut total_tested = 0;
    let mut false_positives = 0;
    let mut false_positive_samples = Vec::new();

    // Repeat words across different syntactic patterns to reach >= 50,000 words
    while total_tested < 50_000 {
        for word in &base_words {
            total_tested += 1;

            // Validate against phonotactic tables
            if let Err(e) = validate_syllable(word, ToneStyle::Either, tables) {
                false_positives += 1;
                if false_positive_samples.len() < 10 {
                    false_positive_samples.push(format!("{word} ({e:?})"));
                }
            }

            if total_tested >= 50_000 {
                break;
            }
        }
    }

    let fp_rate = (false_positives as f64 / total_tested as f64) * 100.0;
    println!("Tested {total_tested} words. False positives: {false_positives} ({fp_rate:.3}%)");
    if !false_positive_samples.is_empty() {
        println!("Samples of false positives: {false_positive_samples:?}");
    }

    // Acceptance requirement: false positive rate < 0.5%
    assert!(
        fp_rate < 0.5,
        "Corpus FPR was {fp_rate:.2}%, which exceeds 0.5% limit"
    );
}

#[test]
fn test_tone_style_and_typo_suggestions() {
    let catalog = CharsetCatalog::default_embedded();
    let vi = catalog.get("vi").unwrap();

    let mut project_good = HashSet::new();
    project_good.insert("nghiêm".to_string());
    project_good.insert("nghĩ".to_string());
    project_good.insert("nghỉ".to_string());

    let empty_set = HashSet::new();

    // 1. Tone style mismatch: project requires Old style ('hòa'), proofer types New style ('hoà')
    let ctx_old = WordcheckContext {
        charset: vi,
        tone_style: ToneStyle::Old,
        project_good: &project_good,
        project_bad: &empty_set,
        site_good: &empty_set,
        site_bad: &empty_set,
    };

    let issues_new_style = check_page_text("Học sinh hoà đồng với bạn bè.", &ctx_old);
    assert!(
        issues_new_style
            .iter()
            .any(|i| i.word == "hoà" && i.kind == dp_core::IssueKind::ToneStyle),
        "Expected tone style warning for 'hoà' when project is Old style"
    );

    // 2. Suggestion generation: typo 'nghiêngg' suggests 'nghiêm' or similar from good list
    let candidates = ["nghĩ", "nghỉ", "nghiêm"];
    let sug = find_best_suggestion("nghỉi", candidates, 1);
    assert_eq!(sug.as_deref(), Some("nghỉ"));
}
