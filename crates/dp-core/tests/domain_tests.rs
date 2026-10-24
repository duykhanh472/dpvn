use dp_core::{DomainError, PageId, PageState, Phase, ProjectId, RoundId, ToneStyle, UserId};
use std::str::FromStr;

#[test]
fn test_ids() {
    let u = UserId::new(42);
    assert_eq!(u.get(), 42);
    assert_eq!(u.to_string(), "42");
    assert_eq!(u, UserId::from(42));

    let p = ProjectId::new(101);
    assert_eq!(p.get(), 101);
    assert_eq!(p.to_string(), "101");

    let page = PageId::new(999);
    assert_eq!(page.get(), 999);
    assert_eq!(page.to_string(), "999");
}

#[test]
fn test_phase_conversions() {
    let phases = [
        (Phase::Draft, "draft"),
        (Phase::Queued, "queued"),
        (Phase::InRound, "in_round"),
        (Phase::PostProcessing, "post_processing"),
        (Phase::Posted, "posted"),
        (Phase::Archived, "archived"),
    ];

    for (phase, s) in phases {
        assert_eq!(phase.as_str(), s);
        assert_eq!(phase.to_string(), s);
        assert_eq!(Phase::from_str(s).unwrap(), phase);
    }

    assert!(matches!(
        Phase::from_str("nonexistent"),
        Err(DomainError::InvalidInput(_))
    ));
}

#[test]
fn test_tone_style_conversions() {
    assert_eq!(ToneStyle::Old.as_str(), "old");
    assert_eq!(ToneStyle::New.as_str(), "new");
    assert_eq!(ToneStyle::Either.as_str(), "either");
    assert_eq!(ToneStyle::from_str("old").unwrap(), ToneStyle::Old);
    assert_eq!(ToneStyle::from_str("new").unwrap(), ToneStyle::New);
    assert_eq!(ToneStyle::from_str("either").unwrap(), ToneStyle::Either);
}

#[test]
fn test_round_id() {
    let r1 = RoundId::new("P1").unwrap();
    assert_eq!(r1.as_str(), "P1");
    assert_eq!(r1.to_string(), "P1");

    assert!(RoundId::new("").is_err());
    assert!(RoundId::new("   ").is_err());
    assert!(RoundId::new("TOOLONGROUND").is_err());
}

#[test]
fn test_page_state() {
    assert_eq!(PageState::Available.as_str(), "available");
    assert_eq!(PageState::InProgress.as_str(), "in_progress");
    assert_eq!(PageState::Done.as_str(), "done");
    assert_eq!(
        PageState::from_str("available").unwrap(),
        PageState::Available
    );
}
