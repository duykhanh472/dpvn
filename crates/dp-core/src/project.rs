//! Project state and configuration domain models.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::DomainError;

/// High-level lifecycle phase of a proofreading project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Newly created project; pages and OCR text are being uploaded and validated.
    Draft,
    /// Ready for work; waiting in queue for proofreaders.
    Queued,
    /// Currently active in a proofreading or formatting round (e.g. P1..F2).
    InRound,
    /// All rounds completed; undergoing final post-processing.
    PostProcessing,
    /// Finished and posted to the repository/catalog.
    Posted,
    /// Closed or archived project.
    Archived,
}

impl Phase {
    /// Return the canonical string representation matching database ENUM values.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Queued => "queued",
            Self::InRound => "in_round",
            Self::PostProcessing => "post_processing",
            Self::Posted => "posted",
            Self::Archived => "archived",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Phase {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "draft" => Ok(Self::Draft),
            "queued" => Ok(Self::Queued),
            "in_round" => Ok(Self::InRound),
            "post_processing" => Ok(Self::PostProcessing),
            "posted" => Ok(Self::Posted),
            "archived" => Ok(Self::Archived),
            other => Err(DomainError::InvalidInput(format!(
                "unknown phase: '{other}'"
            ))),
        }
    }
}

/// Vietnamese tone placement convention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneStyle {
    /// Kiểu cũ (traditional tone placement, e.g., `hòa`, `thúy`).
    Old,
    /// Kiểu mới (modern tone placement, e.g., `hoà`, `thuý`).
    New,
    /// Accept either style without flagging consistency warnings.
    #[default]
    Either,
}

impl ToneStyle {
    /// Return the canonical string representation matching database ENUM values.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Old => "old",
            Self::New => "new",
            Self::Either => "either",
        }
    }
}

impl fmt::Display for ToneStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ToneStyle {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "old" => Ok(Self::Old),
            "new" => Ok(Self::New),
            "either" => Ok(Self::Either),
            other => Err(DomainError::InvalidInput(format!(
                "unknown tone style: '{other}'"
            ))),
        }
    }
}
