//! Runtime site settings loaded from the `settings` table (Phase 10).
//!
//! The database is the source of truth for values the admin can tune at
//! runtime. Missing keys fall back to `config.toml`, so a fresh install works
//! with no rows in `settings`.

use std::collections::HashMap;

use dp_db::{DbError, queries::settings};
use sqlx::MySqlPool;

use crate::config::LimitsConfig;

/// Effective site settings for the current request or job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteSettings {
    /// Whether self-service registration is open.
    pub registration_open: bool,
    /// Daily proofing page limit per user.
    pub daily_pages: u32,
    /// Hours before an unsaved page claim is released.
    pub claim_timeout_hours: u32,
}

impl SiteSettings {
    /// Build effective settings from stored key/value pairs and config defaults.
    #[must_use]
    pub fn from_map(map: &HashMap<String, String>, defaults: &LimitsConfig) -> Self {
        let registration_open = map
            .get(settings::KEY_REGISTRATION_OPEN)
            .is_none_or(|value| parse_bool(value));

        let daily_pages = map
            .get(settings::KEY_DAILY_PAGES)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(defaults.daily_pages);

        let claim_timeout_hours = map
            .get(settings::KEY_CLAIM_TIMEOUT_HOURS)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(defaults.claim_timeout_hours);

        Self {
            registration_open,
            daily_pages,
            claim_timeout_hours,
        }
    }

    /// Load effective settings from the database, falling back to config defaults.
    ///
    /// # Errors
    /// Returns [`DbError::Sqlx`] if the settings query fails.
    pub async fn load(pool: &MySqlPool, defaults: &LimitsConfig) -> Result<Self, DbError> {
        let map = settings::get_all_settings(pool).await?;
        Ok(Self::from_map(&map, defaults))
    }

    /// Build settings purely from config defaults (used by CLI fallbacks).
    #[must_use]
    pub fn from_defaults(defaults: &LimitsConfig) -> Self {
        Self {
            registration_open: true,
            daily_pages: defaults.daily_pages,
            claim_timeout_hours: defaults.claim_timeout_hours,
        }
    }
}

/// Parse a stored boolean, defaulting to `true` for unrecognized values.
fn parse_bool(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> LimitsConfig {
        LimitsConfig {
            max_upload_mb: 512,
            daily_pages: 100,
            claim_timeout_hours: 72,
        }
    }

    #[test]
    fn missing_keys_use_defaults() {
        let settings = SiteSettings::from_map(&HashMap::new(), &defaults());
        assert!(settings.registration_open);
        assert_eq!(settings.daily_pages, 100);
        assert_eq!(settings.claim_timeout_hours, 72);
    }

    #[test]
    fn stored_values_override_defaults() {
        let map = HashMap::from([
            (settings::KEY_REGISTRATION_OPEN.to_string(), "0".to_string()),
            (settings::KEY_DAILY_PAGES.to_string(), "25".to_string()),
            (
                settings::KEY_CLAIM_TIMEOUT_HOURS.to_string(),
                "12".to_string(),
            ),
        ]);

        let settings = SiteSettings::from_map(&map, &defaults());
        assert!(!settings.registration_open);
        assert_eq!(settings.daily_pages, 25);
        assert_eq!(settings.claim_timeout_hours, 12);
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let map = HashMap::from([
            (settings::KEY_DAILY_PAGES.to_string(), "0".to_string()),
            (
                settings::KEY_CLAIM_TIMEOUT_HOURS.to_string(),
                "not-a-number".to_string(),
            ),
        ]);

        let settings = SiteSettings::from_map(&map, &defaults());
        assert_eq!(settings.daily_pages, 100);
        assert_eq!(settings.claim_timeout_hours, 72);
    }
}
