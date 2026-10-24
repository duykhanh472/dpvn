//! Database queries for user sessions and CSRF token management.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, MySqlPool};

use crate::DbError;

/// Projection representing an authenticated session joined with user details.
#[derive(Debug, Clone, FromRow)]
pub struct SessionUser {
    /// ID of authenticated user.
    pub user_id: u64,
    /// Username.
    pub username: String,
    /// Email address.
    pub email: String,
    /// Real name / display name.
    pub real_name: String,
    /// User status.
    pub status: String,
    /// CSRF token bound to this session.
    pub csrf_token: String,
    /// Absolute expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Last seen activity timestamp.
    pub last_seen_at: DateTime<Utc>,
}

/// Create a new session record.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn create_session(
    pool: &MySqlPool,
    token_hash: &[u8],
    user_id: u64,
    csrf_token: &str,
    ip: Option<&[u8]>,
    user_agent: &str,
) -> Result<(), DbError> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, csrf_token, created_at, last_seen_at, expires_at, ip, user_agent)
         VALUES (?, ?, ?, ?, ?, ? + INTERVAL 30 DAY, ?, ?)",
    )
    .bind(token_hash)
    .bind(user_id)
    .bind(csrf_token)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(ip)
    .bind(user_agent)
    .execute(pool)
    .await?;

    Ok(())
}

/// Look up an active, unexpired session by the SHA256 hash of its token.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn lookup_session(
    pool: &MySqlPool,
    token_hash: &[u8],
) -> Result<Option<SessionUser>, DbError> {
    let row = sqlx::query_as::<_, SessionUser>(
        "SELECT s.user_id, u.username, u.email, u.real_name, u.status, s.csrf_token, s.expires_at, s.last_seen_at
         FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ? AND s.expires_at > NOW(6) AND u.status = 'active'",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Update `last_seen_at` for an active session, throttled to at most once per 5 minutes.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn touch_session(pool: &MySqlPool, token_hash: &[u8]) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE sessions
         SET last_seen_at = NOW(6)
         WHERE token_hash = ? AND last_seen_at < NOW(6) - INTERVAL 5 MINUTE",
    )
    .bind(token_hash)
    .execute(pool)
    .await?;

    Ok(())
}

/// Delete a session on logout.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn delete_session(pool: &MySqlPool, token_hash: &[u8]) -> Result<bool, DbError> {
    let res = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(pool)
        .await?;

    Ok(res.rows_affected() > 0)
}

/// Invalidate all active sessions for a user (e.g., after password change).
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn delete_user_sessions(pool: &MySqlPool, user_id: u64) -> Result<u64, DbError> {
    let res = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(res.rows_affected())
}

/// Prune all expired sessions from the database.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn prune_expired_sessions(pool: &MySqlPool) -> Result<u64, DbError> {
    let res = sqlx::query("DELETE FROM sessions WHERE expires_at < NOW(6)")
        .execute(pool)
        .await?;

    Ok(res.rows_affected())
}
