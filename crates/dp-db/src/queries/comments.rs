//! Project comment queries (`project_comments`).
//!
//! Comments are rendered and sanitized by `dp_core::markdown` before insertion;
//! both the Markdown source and the rendered HTML are stored (§7.2).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};

use crate::DbError;

/// A comment row with its author's username, ready for display.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectCommentItem {
    /// Comment ID.
    pub id: u64,
    /// Sanitized HTML rendered at write time.
    pub body_html: String,
    /// Username of the comment author.
    pub author_username: String,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
}

/// Append a comment to a project.
///
/// The caller is responsible for normalizing and rendering `body_md` first.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the insert fails.
pub async fn insert_project_comment(
    pool: &MySqlPool,
    project_id: u64,
    author_id: u64,
    body_md: &str,
    body_html: &str,
) -> Result<u64, DbError> {
    let res = sqlx::query(
        "INSERT INTO project_comments (project_id, author_id, body_md, body_html, created_at)
         VALUES (?, ?, ?, ?, UTC_TIMESTAMP(6))",
    )
    .bind(project_id)
    .bind(author_id)
    .bind(body_md)
    .bind(body_html)
    .execute(pool)
    .await?;

    Ok(res.last_insert_id())
}

/// List the newest comments for a project, returned oldest-first for display.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn list_project_comments(
    pool: &MySqlPool,
    project_id: u64,
    limit: u32,
    offset: u32,
) -> Result<Vec<ProjectCommentItem>, DbError> {
    let mut rows = sqlx::query_as::<_, ProjectCommentItem>(
        "SELECT pc.id, pc.body_html, u.username AS author_username, pc.created_at
         FROM project_comments pc
         JOIN users u ON u.id = pc.author_id
         WHERE pc.project_id = ?
         ORDER BY pc.created_at DESC, pc.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(project_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    // The query selects newest-first for the LIMIT window; display oldest-first.
    rows.reverse();
    Ok(rows)
}

/// Count comments on a project.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn count_project_comments(pool: &MySqlPool, project_id: u64) -> Result<i64, DbError> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM project_comments WHERE project_id = ?")
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    Ok(count)
}
