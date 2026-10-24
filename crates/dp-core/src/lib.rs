//! Pure domain crate containing entities, state machines, types, and domain rules.
//!
//! This crate does NOT depend on axum, sqlx, tokio, or the filesystem.

pub mod error;
pub mod ids;
pub mod project;
pub mod round;

pub use error::DomainError;
pub use ids::{PageId, ProjectId, UserId};
pub use project::{Phase, ToneStyle};
pub use round::{PageState, RoundConfig, RoundId, RoundKind};
