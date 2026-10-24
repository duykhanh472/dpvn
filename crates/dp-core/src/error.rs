//! Domain error definitions.

/// Domain-level errors that represent invalid state transitions, missing entities,
/// or rule violations in the proofreading engine.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// The specified project was not found.
    #[error("project {0} not found")]
    ProjectNotFound(u64),

    /// The requested page is unavailable for claiming.
    #[error("page is not available")]
    PageUnavailable,

    /// The round has unfinished pages and cannot advance.
    #[error("round {round} is not complete")]
    RoundIncomplete {
        /// Identifier of the incomplete round.
        round: String,
    },

    /// The requested state transition is not allowed for the given actor.
    #[error("transition {from} -> {to} is not permitted for {actor}")]
    TransitionDenied {
        /// Originating phase or round.
        from: String,
        /// Destination phase or round.
        to: String,
        /// Role or identity attempting the transition.
        actor: String,
    },

    /// The user has reached their daily page proofing limit.
    #[error("daily limit of {limit} pages reached")]
    DailyLimitReached {
        /// Configured daily limit.
        limit: u32,
    },

    /// General invalid input or domain validation failure.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Missing or invalid configuration.
    #[error("configuration error: {0}")]
    Config(String),
}
