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

/// Embedded default rounds configuration from rounds.toml.
pub const DEFAULT_ROUNDS_TOML: &str = include_str!("rounds.toml");

/// Top-level rounds configuration loaded from `rounds.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundsConfig {
    /// Ordered list of round specifications.
    #[serde(rename = "round", default)]
    pub rounds: Vec<RoundConfig>,
}

impl RoundsConfig {
    /// Load and validate the default embedded `rounds.toml`.
    ///
    /// # Panics
    /// Panics if the embedded `rounds.toml` is invalid (guaranteed valid at build time).
    #[must_use]
    pub fn default_embedded() -> Self {
        Self::parse(DEFAULT_ROUNDS_TOML).expect("embedded rounds.toml must be valid")
    }

    /// Parse a TOML string into a [`RoundsConfig`] and validate all domain invariants.
    ///
    /// # Errors
    /// Returns [`DomainError::Config`] if TOML syntax is malformed or domain invariants fail.
    pub fn parse(toml_str: &str) -> Result<Self, DomainError> {
        let config: Self = toml::from_str(toml_str)
            .map_err(|e| DomainError::Config(format!("failed to parse rounds.toml: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the rounds configuration against business invariants:
    /// - At least one round defined.
    /// - All round IDs are unique, non-empty, and <= 8 characters.
    /// - Exactly one round has `min_pages = 0` (which must be the first round).
    /// - `min_pages` is non-decreasing across subsequent rounds.
    /// - Every round specifies a non-empty `charset`.
    ///
    /// # Errors
    /// Returns [`DomainError::Config`] with a descriptive message if any invariant is violated.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.rounds.is_empty() {
            return Err(DomainError::Config(
                "rounds configuration must define at least one round".to_string(),
            ));
        }

        let mut seen_ids = std::collections::HashSet::new();
        let mut zero_min_pages_count = 0usize;
        let mut prev_min_pages = 0u32;

        for (idx, r) in self.rounds.iter().enumerate() {
            let id_str = r.id.as_str();
            if id_str.trim().is_empty() {
                return Err(DomainError::Config(format!("round #{idx} has an empty id")));
            }
            if id_str.chars().count() > 8 {
                return Err(DomainError::Config(format!(
                    "round ID '{id_str}' exceeds maximum allowed length of 8 chars"
                )));
            }
            if !seen_ids.insert(r.id.clone()) {
                return Err(DomainError::Config(format!(
                    "duplicate round id '{id_str}'"
                )));
            }

            if r.charset.trim().is_empty() {
                return Err(DomainError::Config(format!(
                    "round '{id_str}' specifies an empty charset"
                )));
            }

            if r.min_pages == 0 {
                zero_min_pages_count += 1;
                if idx != 0 {
                    return Err(DomainError::Config(format!(
                        "only the first round may have min_pages = 0, but round '{id_str}' (position {idx}) has min_pages = 0"
                    )));
                }
            } else if r.min_pages < prev_min_pages {
                return Err(DomainError::Config(format!(
                    "round '{id_str}' has min_pages = {}, which is less than previous round min_pages = {prev_min_pages}",
                    r.min_pages
                )));
            }

            prev_min_pages = r.min_pages;
        }

        if zero_min_pages_count != 1 {
            return Err(DomainError::Config(format!(
                "exactly one round must have min_pages = 0, found {zero_min_pages_count}"
            )));
        }

        Ok(())
    }

    /// Retrieve the first round in progression.
    #[must_use]
    pub fn first_round(&self) -> Option<&RoundConfig> {
        self.rounds.first()
    }

    /// Retrieve the last round in progression.
    #[must_use]
    pub fn last_round(&self) -> Option<&RoundConfig> {
        self.rounds.last()
    }

    /// Find a round by its identifier.
    #[must_use]
    pub fn get_round(&self, id: &RoundId) -> Option<&RoundConfig> {
        self.rounds.iter().find(|r| &r.id == id)
    }

    /// Retrieve the next round succeeding `current` in progression.
    #[must_use]
    pub fn next_round(&self, current: &RoundId) -> Option<&RoundConfig> {
        let pos = self.rounds.iter().position(|r| &r.id == current)?;
        self.rounds.get(pos + 1)
    }

    /// Retrieve the previous round preceding `current` in progression.
    #[must_use]
    pub fn prev_round(&self, current: &RoundId) -> Option<&RoundConfig> {
        let pos = self.rounds.iter().position(|r| &r.id == current)?;
        if pos == 0 {
            None
        } else {
            self.rounds.get(pos - 1)
        }
    }

    /// Return true if `id` is the final round.
    #[must_use]
    pub fn is_last_round(&self, id: &RoundId) -> bool {
        self.last_round().is_some_and(|r| &r.id == id)
    }

    /// Return true if `id` is the initial round.
    #[must_use]
    pub fn is_first_round(&self, id: &RoundId) -> bool {
        self.first_round().is_some_and(|r| &r.id == id)
    }

    /// Check whether a user with `user_proofed_pages` is permitted to work on round `id`.
    #[must_use]
    pub fn is_accessible(&self, id: &RoundId, user_proofed_pages: u32) -> bool {
        self.get_round(id)
            .is_some_and(|r| r.enabled && user_proofed_pages >= r.min_pages)
    }
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_rounds_config_valid() {
        let config = RoundsConfig::default_embedded();
        assert_eq!(config.rounds.len(), 5);
        assert_eq!(config.first_round().unwrap().id.as_str(), "P1");
        assert_eq!(config.last_round().unwrap().id.as_str(), "F2");
        assert!(config.is_first_round(&RoundId::new("P1").unwrap()));
        assert!(config.is_last_round(&RoundId::new("F2").unwrap()));

        let p1 = RoundId::new("P1").unwrap();
        let p2 = RoundId::new("P2").unwrap();
        let f2 = RoundId::new("F2").unwrap();

        assert_eq!(config.next_round(&p1).unwrap().id.as_str(), "P2");
        assert_eq!(config.prev_round(&p2).unwrap().id.as_str(), "P1");
        assert!(config.next_round(&f2).is_none());
        assert!(config.prev_round(&p1).is_none());
    }

    #[test]
    fn test_round_accessibility_thresholds() {
        let config = RoundsConfig::default_embedded();
        let p1 = RoundId::new("P1").unwrap();
        let p2 = RoundId::new("P2").unwrap();
        let f2 = RoundId::new("F2").unwrap();

        assert!(config.is_accessible(&p1, 0));
        assert!(!config.is_accessible(&p2, 19));
        assert!(config.is_accessible(&p2, 20));
        assert!(!config.is_accessible(&f2, 399));
        assert!(config.is_accessible(&f2, 400));
    }

    #[test]
    fn test_reject_empty_rounds() {
        let toml_str = "round = []";
        let res = RoundsConfig::parse(toml_str);
        assert!(res.is_err());
    }

    #[test]
    fn test_reject_duplicate_ids() {
        let toml_str = r#"
[[round]]
id = "P1"
kind = "proof"
charset = "vi"
min_pages = 0

[[round]]
id = "P1"
kind = "proof"
charset = "vi"
min_pages = 20
"#;
        let res = RoundsConfig::parse(toml_str);
        assert!(res.is_err());
    }

    #[test]
    fn test_reject_decreasing_min_pages() {
        let toml_str = r#"
[[round]]
id = "P1"
kind = "proof"
charset = "vi"
min_pages = 0

[[round]]
id = "P2"
kind = "proof"
charset = "vi"
min_pages = 50

[[round]]
id = "P3"
kind = "proof"
charset = "vi"
min_pages = 30
"#;
        let res = RoundsConfig::parse(toml_str);
        assert!(res.is_err());
    }

    #[test]
    fn test_reject_multiple_zero_min_pages() {
        let toml_str = r#"
[[round]]
id = "P1"
kind = "proof"
charset = "vi"
min_pages = 0

[[round]]
id = "P2"
kind = "proof"
charset = "vi"
min_pages = 0
"#;
        let res = RoundsConfig::parse(toml_str);
        assert!(res.is_err());
    }
}
