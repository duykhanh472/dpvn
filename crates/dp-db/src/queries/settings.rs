//! Queries for the `settings` key-value store (§7.2, Phase 10).
//!
//! Runtime-tunable site settings are stored here so operators can change them
//! from the admin UI without editing `config.toml` or restarting the binary.
//! Missing keys fall back to the caller's compiled configuration defaults.

use std::collections::HashMap;

use chrono::Utc;
use sqlx::MySqlPool;

use crate::DbError;

/// Whether self-service registration is open (`1`/`0`).
pub const KEY_REGISTRATION_OPEN: &str = "registration_open";
/// Daily proofing page limit (`u32`).
pub const KEY_DAILY_PAGES: &str = "daily_pages";
/// Hours before an unsaved page claim is released (`u32`).
pub const KEY_CLAIM_TIMEOUT_HOURS: &str = "claim_timeout_hours";

/// Read a single setting value.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn get_setting(pool: &MySqlPool, key: &str) -> Result<Option<String>, DbError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT v FROM settings WHERE k = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|(value,)| value))
}

/// Load every setting into a map.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn get_all_settings(pool: &MySqlPool) -> Result<HashMap<String, String>, DbError> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT k, v FROM settings")
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().collect())
}

/// Insert or update one setting.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the statement fails.
pub async fn upsert_setting(pool: &MySqlPool, key: &str, value: &str) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO settings (k, v, updated_at)
         VALUES (?, ?, ?)
         ON DUPLICATE KEY UPDATE v = VALUES(v), updated_at = VALUES(updated_at)",
    )
    .bind(key)
    .bind(value)
    .bind(Utc::now())
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert or update several settings atomically.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if any statement fails.
pub async fn upsert_settings(pool: &MySqlPool, values: &[(&str, String)]) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let now = Utc::now();

    for (key, value) in values {
        sqlx::query(
            "INSERT INTO settings (k, v, updated_at)
             VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE v = VALUES(v), updated_at = VALUES(updated_at)",
        )
        .bind(key)
        .bind(value)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Delete one setting, restoring the compiled default on the next read.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the statement fails.
pub async fn delete_setting(pool: &MySqlPool, key: &str) -> Result<bool, DbError> {
    let res = sqlx::query("DELETE FROM settings WHERE k = ?")
        .bind(key)
        .execute(pool)
        .await?;

    Ok(res.rows_affected() > 0)
}
