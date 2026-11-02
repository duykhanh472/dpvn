//! Audit trail for administrative actions (Phase 10).
//!
//! Project lifecycle changes are already recorded in `project_events`; this
//! table captures actions that are not project-scoped, such as role grants,
//! account disabling, settings changes, and dictionary maintenance.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};

use crate::DbError;

/// One audited administrative action.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AdminActionItem {
    /// Unique audit entry ID.
    pub id: u64,
    /// Acting user ID, or `None` if the account was later deleted.
    pub actor_id: Option<u64>,
    /// Username snapshot captured at write time.
    pub actor_username: String,
    /// Stable action code (e.g. `user.role.grant`).
    pub action: String,
    /// Target entity type (e.g. `user`, `project`, `site_word`, `settings`).
    pub target_type: String,
    /// Target entity ID, when applicable.
    pub target_id: Option<u64>,
    /// Human-readable context for the action.
    pub detail: String,
    /// Event timestamp (UTC).
    pub created_at: DateTime<Utc>,
}

/// DTO for appending an administrative audit entry.
#[derive(Debug, Clone)]
pub struct NewAdminAction<'a> {
    /// Acting user ID.
    pub actor_id: u64,
    /// Acting username snapshot.
    pub actor_username: &'a str,
    /// Stable action code.
    pub action: &'a str,
    /// Target entity type.
    pub target_type: &'a str,
    /// Target entity ID, when applicable.
    pub target_id: Option<u64>,
    /// Human-readable context.
    pub detail: &'a str,
}

/// Append an administrative audit entry.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the insert fails.
pub async fn record_admin_action(
    pool: &MySqlPool,
    action: &NewAdminAction<'_>,
) -> Result<u64, DbError> {
    let res = sqlx::query(
        "INSERT INTO admin_audit_log (
            actor_id, actor_username, action, target_type, target_id, detail, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(action.actor_id)
    .bind(action.actor_username)
    .bind(action.action)
    .bind(action.target_type)
    .bind(action.target_id)
    .bind(action.detail)
    .bind(Utc::now())
    .execute(pool)
    .await?;

    Ok(res.last_insert_id())
}

/// List administrative audit entries, newest first.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn list_admin_actions(
    pool: &MySqlPool,
    limit: u32,
    offset: u32,
) -> Result<Vec<AdminActionItem>, DbError> {
    let rows = sqlx::query_as::<_, AdminActionItem>(
        "SELECT id, actor_id, actor_username, action, target_type, target_id,
                detail, created_at
         FROM admin_audit_log
         ORDER BY created_at DESC, id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Count administrative audit entries.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn count_admin_actions(pool: &MySqlPool) -> Result<i64, DbError> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admin_audit_log")
        .fetch_one(pool)
        .await?;

    Ok(count)
}
