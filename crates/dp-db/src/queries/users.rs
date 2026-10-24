//! Database queries for user accounts and role memberships.

use chrono::Utc;
use sqlx::MySqlPool;

use crate::DbError;
use crate::models::User;

/// Create a new pending user with an email activation token and default 'proofer' role.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database execution fails (e.g., unique key violation on username or email).
pub async fn create_user(
    pool: &MySqlPool,
    username: &str,
    email: &str,
    password_hash: &str,
    real_name: &str,
    activate_token: &str,
) -> Result<u64, DbError> {
    let mut tx = pool.begin().await?;

    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO users (username, email, password_hash, real_name, status, activate_token, created_at)
         VALUES (?, ?, ?, ?, 'pending', ?, ?)",
    )
    .bind(username)
    .bind(email)
    .bind(password_hash)
    .bind(real_name)
    .bind(activate_token)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let user_id = res.last_insert_id();

    // Default role for volunteers
    sqlx::query(
        "INSERT INTO user_roles (user_id, role, granted_at)
         VALUES (?, 'proofer', ?)",
    )
    .bind(user_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(user_id)
}

/// Create an active user with 'admin' and 'proofer' roles (used by CLI create-admin).
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database execution fails.
pub async fn create_admin_user(
    pool: &MySqlPool,
    username: &str,
    email: &str,
    password_hash: &str,
) -> Result<u64, DbError> {
    let mut tx = pool.begin().await?;

    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO users (username, email, password_hash, real_name, status, activate_token, created_at)
         VALUES (?, ?, ?, '', 'active', NULL, ?)",
    )
    .bind(username)
    .bind(email)
    .bind(password_hash)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let user_id = res.last_insert_id();

    sqlx::query(
        "INSERT INTO user_roles (user_id, role, granted_at)
         VALUES (?, 'proofer', ?), (?, 'admin', ?)",
    )
    .bind(user_id)
    .bind(now)
    .bind(user_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(user_id)
}

/// Retrieve a user by their unique primary key ID.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_user_by_id(pool: &MySqlPool, user_id: u64) -> Result<Option<User>, DbError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, email, password_hash, real_name, status, activate_token, created_at, last_login_at
         FROM users
         WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

/// Retrieve a user by username (case/accent-insensitive lookup via utf8mb4_0900_ai_ci).
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_user_by_username(
    pool: &MySqlPool,
    username: &str,
) -> Result<Option<User>, DbError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, email, password_hash, real_name, status, activate_token, created_at, last_login_at
         FROM users
         WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

/// Retrieve a user by email (case/accent-insensitive lookup via utf8mb4_0900_ai_ci).
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_user_by_email(pool: &MySqlPool, email: &str) -> Result<Option<User>, DbError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, email, password_hash, real_name, status, activate_token, created_at, last_login_at
         FROM users
         WHERE email = ?",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

/// Retrieve a pending user by activation token.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_user_by_activation_token(
    pool: &MySqlPool,
    token: &str,
) -> Result<Option<User>, DbError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, email, password_hash, real_name, status, activate_token, created_at, last_login_at
         FROM users
         WHERE activate_token = ? AND status = 'pending'",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

/// Activate an account using its token, setting status to 'active' and clearing the token.
/// Returns true if a pending account was found and activated.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn activate_user(pool: &MySqlPool, token: &str) -> Result<bool, DbError> {
    let res = sqlx::query(
        "UPDATE users
         SET status = 'active', activate_token = NULL
         WHERE activate_token = ? AND status = 'pending'",
    )
    .bind(token)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Update the `last_login_at` timestamp of an active user.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn update_last_login(pool: &MySqlPool, user_id: u64) -> Result<(), DbError> {
    let now = Utc::now();
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(now)
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Get all assigned roles for a user.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn get_user_roles(pool: &MySqlPool, user_id: u64) -> Result<Vec<String>, DbError> {
    let rows = sqlx::query_as::<_, (String,)>("SELECT role FROM user_roles WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(|(r,)| r).collect())
}

/// Delete pending users who have not verified their email after `older_than_days` days.
///
/// # Errors
/// Returns [`DbError::Sqlx`] on query failure.
pub async fn prune_pending_users(pool: &MySqlPool, older_than_days: u32) -> Result<u64, DbError> {
    let res = sqlx::query(
        "DELETE FROM users
         WHERE status = 'pending' AND created_at < NOW(6) - INTERVAL ? DAY",
    )
    .bind(older_than_days)
    .execute(pool)
    .await?;

    Ok(res.rows_affected())
}
