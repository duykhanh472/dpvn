//! Project lifecycle audit trail queries (`project_events`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySql, MySqlPool, Transaction};

use crate::DbError;
use crate::models::ProjectEvent;

/// Audit event enriched with the acting user's username.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectEventItem {
    /// Unique event ID.
    pub id: u64,
    /// Previous phase.
    pub from_phase: Option<String>,
    /// New phase.
    pub to_phase: Option<String>,
    /// Previous round.
    pub from_round: Option<String>,
    /// New round.
    pub to_round: Option<String>,
    /// Username of the actor, or `None` for automatic transitions.
    pub actor_username: Option<String>,
    /// Contextual note.
    pub note: String,
    /// Event timestamp.
    pub created_at: DateTime<Utc>,
}

/// DTO for appending a new project audit event.
#[derive(Debug, Clone)]
pub struct NewProjectEvent<'a> {
    /// Target project ID.
    pub project_id: u64,
    /// Previous phase.
    pub from_phase: Option<&'a str>,
    /// New phase.
    pub to_phase: Option<&'a str>,
    /// Previous round.
    pub from_round: Option<&'a str>,
    /// New round.
    pub to_round: Option<&'a str>,
    /// Initiating user ID (None if automatic system action).
    pub actor_id: Option<u64>,
    /// Event description.
    pub note: &'a str,
}

/// Append a project event within an existing transaction.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database insert fails.
pub async fn record_event_tx(
    tx: &mut Transaction<'_, MySql>,
    event: &NewProjectEvent<'_>,
) -> Result<u64, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO project_events (
            project_id, from_phase, to_phase, from_round, to_round,
            actor_id, note, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.project_id)
    .bind(event.from_phase)
    .bind(event.to_phase)
    .bind(event.from_round)
    .bind(event.to_round)
    .bind(event.actor_id)
    .bind(event.note)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    Ok(res.last_insert_id())
}

/// Append a project event using a connection pool.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database insert fails.
pub async fn record_event(pool: &MySqlPool, event: &NewProjectEvent<'_>) -> Result<u64, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO project_events (
            project_id, from_phase, to_phase, from_round, to_round,
            actor_id, note, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.project_id)
    .bind(event.from_phase)
    .bind(event.to_phase)
    .bind(event.from_round)
    .bind(event.to_round)
    .bind(event.actor_id)
    .bind(event.note)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(res.last_insert_id())
}

/// Retrieve audit history for a project, sorted newest first.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn list_project_events(
    pool: &MySqlPool,
    project_id: u64,
    limit: u32,
    offset: u32,
) -> Result<Vec<ProjectEvent>, DbError> {
    let rows = sqlx::query_as::<_, ProjectEvent>(
        "SELECT id, project_id, from_phase, to_phase, from_round, to_round,
                actor_id, note, created_at
         FROM project_events
         WHERE project_id = ?
         ORDER BY created_at DESC, id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(project_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Retrieve audit history for a project with actor usernames, newest first.
///
/// Visibility is restricted by the caller: per plan Q6 only managers and admins
/// may see the event log.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn list_project_events_with_actors(
    pool: &MySqlPool,
    project_id: u64,
    limit: u32,
) -> Result<Vec<ProjectEventItem>, DbError> {
    let rows = sqlx::query_as::<_, ProjectEventItem>(
        "SELECT pe.id, pe.from_phase, pe.to_phase, pe.from_round, pe.to_round,
                u.username AS actor_username, pe.note, pe.created_at
         FROM project_events pe
         LEFT JOIN users u ON u.id = pe.actor_id
         WHERE pe.project_id = ?
         ORDER BY pe.created_at DESC, pe.id DESC
         LIMIT ?",
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
