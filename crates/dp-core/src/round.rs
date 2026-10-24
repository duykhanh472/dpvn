//! Round definitions, page states, and round configurations.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::DomainError;

/// Identifier for a proofreading round (e.g. "P1", "P2", "F1").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoundId(String);

impl RoundId {
    /// Create and validate a round identifier.
    ///
    /// # Errors
    /// Returns a [`DomainError::InvalidInput`] if the ID is empty or exceeds 8 characters.
    pub fn new(id: impl Into<String>) -> Result<Self, DomainError> {
        let s = id.into();
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(DomainError::InvalidInput(
                "round id cannot be empty".to_string(),
            ));
        }
        if trimmed.chars().count() > 8 {
            return Err(DomainError::InvalidInput(format!(
                "round id '{trimmed}' exceeds maximum length of 8 characters"
            )));
        }
        Ok(Self(trimmed.to_string()))
    }

    /// Return a string slice of the round ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoundId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for RoundId {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// The state of a single page within a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    /// Available for any qualified proofreader to claim.
    Available,
    /// Currently claimed and being worked on by a proofreader.
    InProgress,
    /// Proofing for this page in this round is finished.
    Done,
}

impl PageState {
    /// Return the canonical string representation matching database ENUM values.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
}

impl fmt::Display for PageState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PageState {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "available" => Ok(Self::Available),
            "in_progress" => Ok(Self::InProgress),
            "done" => Ok(Self::Done),
            other => Err(DomainError::InvalidInput(format!(
                "unknown page state: '{other}'"
            ))),
        }
    }
}

/// Kind of activity conducted in a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundKind {
    /// Proofreading textual accuracy against page scans.
    Proof,
    /// Formatting layout, typography, poetry, and footnotes.
    Format,
}

/// Specification for a single proofreading or formatting round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundConfig {
    /// Unique round identifier (e.g., "P1", "P2").
    pub id: RoundId,
    /// Kind of activity.
    pub kind: RoundKind,
    /// Charset key reference (e.g., "vi").
    pub charset: String,
    /// Minimum lifetime proofed pages required to access this round.
    pub min_pages: u32,
    /// Whether this round is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}
