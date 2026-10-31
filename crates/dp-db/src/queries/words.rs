//! Database queries for site-wide and project-level custom dictionaries (§9.2, §7.2).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};
use std::collections::HashSet;

use crate::DbError;

/// Individual entry in a word list.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct WordEntry {
    /// "good" (suppresses warnings) or "bad" (forces warning)
    pub kind: String,
    /// Syllable or word string (NFC).
    pub word: String,
    /// User ID of contributor who added this entry.
    pub added_by: Option<u64>,
    /// Username of contributor if available.
    pub added_by_username: Option<String>,
    /// Timestamp when word was registered.
    pub added_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct WordRow {
    kind: String,
    word: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Site Words (§9.2, §7.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Load all site-wide good and bad words into in-memory hash sets for rapid lookup (§9.6).
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_all_site_words(
    pool: &MySqlPool,
) -> Result<(HashSet<String>, HashSet<String>), DbError> {
    let rows: Vec<WordRow> = sqlx::query_as("SELECT kind, word FROM site_words")
        .fetch_all(pool)
        .await?;

    let mut good = HashSet::with_capacity(rows.len());
    let mut bad = HashSet::new();

    for row in rows {
        let w = row.word.to_lowercase();
        if row.kind == "good" {
            good.insert(w);
        } else {
            bad.insert(w);
        }
    }

    Ok((good, bad))
}

/// List paginated site words for admin dictionary management UI.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn list_site_words(
    pool: &MySqlPool,
    kind: Option<&str>,
    search: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<WordEntry>, DbError> {
    let search_pattern = search.map(|s| format!("%{s}%"));

    let rows = sqlx::query_as::<_, WordEntry>(
        "SELECT sw.kind, sw.word, sw.added_by, u.username AS added_by_username, sw.added_at
         FROM site_words sw
         LEFT JOIN users u ON u.id = sw.added_by
         WHERE (? IS NULL OR sw.kind = ?)
           AND (? IS NULL OR sw.word LIKE ?)
         ORDER BY sw.word ASC
         LIMIT ? OFFSET ?",
    )
    .bind(kind)
    .bind(kind)
    .bind(&search_pattern)
    .bind(&search_pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Count total site words matching filter criteria.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn count_site_words(
    pool: &MySqlPool,
    kind: Option<&str>,
    search: Option<&str>,
) -> Result<i64, DbError> {
    let search_pattern = search.map(|s| format!("%{s}%"));

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM site_words
         WHERE (? IS NULL OR kind = ?)
           AND (? IS NULL OR word LIKE ?)",
    )
    .bind(kind)
    .bind(kind)
    .bind(&search_pattern)
    .bind(&search_pattern)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

/// Add or update a word in the site-wide dictionary.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn add_site_word(
    pool: &MySqlPool,
    kind: &str,
    word: &str,
    added_by: Option<u64>,
) -> Result<bool, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO site_words (kind, word, added_by, added_at)
         VALUES (?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE added_by = VALUES(added_by), added_at = VALUES(added_at)",
    )
    .bind(kind)
    .bind(word)
    .bind(added_by)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Delete a word from the site-wide dictionary.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn remove_site_word(pool: &MySqlPool, kind: &str, word: &str) -> Result<bool, DbError> {
    let res = sqlx::query("DELETE FROM site_words WHERE kind = ? AND word = ?")
        .bind(kind)
        .bind(word)
        .execute(pool)
        .await?;

    Ok(res.rows_affected() > 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Project Words (§9.2, §7.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Load all custom good and bad words for a specific project.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_all_project_words(
    pool: &MySqlPool,
    project_id: u64,
) -> Result<(HashSet<String>, HashSet<String>), DbError> {
    let rows: Vec<WordRow> =
        sqlx::query_as("SELECT kind, word FROM project_words WHERE project_id = ?")
            .bind(project_id)
            .fetch_all(pool)
            .await?;

    let mut good = HashSet::new();
    let mut bad = HashSet::new();

    for row in rows {
        let w = row.word.to_lowercase();
        if row.kind == "good" {
            good.insert(w);
        } else {
            bad.insert(w);
        }
    }

    Ok((good, bad))
}

/// List paginated project words for project manager word list UI.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn list_project_words(
    pool: &MySqlPool,
    project_id: u64,
    kind: Option<&str>,
    search: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<WordEntry>, DbError> {
    let search_pattern = search.map(|s| format!("%{s}%"));

    let rows = sqlx::query_as::<_, WordEntry>(
        "SELECT pw.kind, pw.word, pw.added_by, u.username AS added_by_username, pw.added_at
         FROM project_words pw
         LEFT JOIN users u ON u.id = pw.added_by
         WHERE pw.project_id = ?
           AND (? IS NULL OR pw.kind = ?)
           AND (? IS NULL OR pw.word LIKE ?)
         ORDER BY pw.word ASC
         LIMIT ? OFFSET ?",
    )
    .bind(project_id)
    .bind(kind)
    .bind(kind)
    .bind(&search_pattern)
    .bind(&search_pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Count total project words matching filter criteria.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn count_project_words(
    pool: &MySqlPool,
    project_id: u64,
    kind: Option<&str>,
    search: Option<&str>,
) -> Result<i64, DbError> {
    let search_pattern = search.map(|s| format!("%{s}%"));

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM project_words
         WHERE project_id = ?
           AND (? IS NULL OR kind = ?)
           AND (? IS NULL OR word LIKE ?)",
    )
    .bind(project_id)
    .bind(kind)
    .bind(kind)
    .bind(&search_pattern)
    .bind(&search_pattern)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

/// Add or update a word in a project's custom dictionary.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn add_project_word(
    pool: &MySqlPool,
    project_id: u64,
    kind: &str,
    word: &str,
    added_by: Option<u64>,
) -> Result<bool, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO project_words (project_id, kind, word, added_by, added_at)
         VALUES (?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE added_by = VALUES(added_by), added_at = VALUES(added_at)",
    )
    .bind(project_id)
    .bind(kind)
    .bind(word)
    .bind(added_by)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Delete a word from a project's custom dictionary.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn remove_project_word(
    pool: &MySqlPool,
    project_id: u64,
    kind: &str,
    word: &str,
) -> Result<bool, DbError> {
    let res = sqlx::query(
        "DELETE FROM project_words
         WHERE project_id = ? AND kind = ? AND word = ?",
    )
    .bind(project_id)
    .bind(kind)
    .bind(word)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}
