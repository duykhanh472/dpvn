//! Pure domain crate containing entities, state machines, types, and domain rules.
//!
//! This crate does NOT depend on axum, sqlx, tokio, or the filesystem.

pub mod charset;
pub mod diff;
pub mod error;
pub mod ids;
pub mod project;
pub mod round;
pub mod transition;
pub mod viet;
pub mod wordcheck;

pub use charset::{Charset, CharsetCatalog};
pub use diff::{DiffOptions, DiffRow, DiffRowKind, diff_html, diff_rows};
pub use error::DomainError;
pub use ids::{PageId, ProjectId, UserId};
pub use project::{Phase, ToneStyle};
pub use round::{PageState, RoundConfig, RoundId, RoundKind, RoundsConfig};
pub use transition::{
    Action, Actor, ActorRole, Effect, Guard, ProjectContext, STANDARD_TRANSITIONS, Transition,
    apply, toggle_on_hold,
};
pub use wordcheck::{
    IssueKind, PhonotacticError, SyllableParts, SyllableToken, Tone, WordIssue, WordcheckContext,
    check_page_text, find_best_suggestion, levenshtein_distance, tokenize_syllables,
    validate_characters, validate_syllable,
};
