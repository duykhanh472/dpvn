//! Database queries for user accounts and role memberships.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySql, MySqlPool, QueryBuilder};

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

// ─────────────────────────────────────────────────────────────────────────────
// Administrative user management (Phase 10)
// ─────────────────────────────────────────────────────────────────────────────

/// User row enriched with its comma-separated role list for the admin UI.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AdminUserRow {
    /// User ID.
    pub id: u64,
    /// Unique username.
    pub username: String,
    /// Email address.
    pub email: String,
    /// Display name.
    pub real_name: String,
    /// Account status ('pending', 'active', 'disabled').
    pub status: String,
    /// Account creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last successful login timestamp.
    pub last_login_at: Option<DateTime<Utc>>,
    /// Comma-separated role names, e.g. `admin,proofer`.
    pub roles: String,
}

/// Filters for the administrative user list.
#[derive(Debug, Clone, Default)]
pub struct AdminUserFilter<'a> {
    /// Substring match over username, email, and display name.
    pub search: Option<&'a str>,
    /// Restrict to users holding this role.
    pub role: Option<&'a str>,
    /// Restrict to this account status.
    pub status: Option<&'a str>,
    /// Page size.
    pub limit: u32,
    /// Row offset.
    pub offset: u32,
}

/// Escape `LIKE` wildcards using `!` as the escape character.
fn like_pattern(input: &str) -> String {
    let mut pattern = String::with_capacity(input.len() + 2);
    pattern.push('%');
    for ch in input.chars() {
        if matches!(ch, '%' | '_' | '!') {
            pattern.push('!');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

/// Append the shared `WHERE` predicates for the admin user list and count.
fn push_admin_user_filters(qb: &mut QueryBuilder<'_, MySql>, filter: &AdminUserFilter<'_>) {
    if let Some(search) = filter.search {
        let pattern = like_pattern(search);
        qb.push(" AND (u.username LIKE ")
            .push_bind(pattern.clone())
            .push(" ESCAPE '!' OR u.email LIKE ")
            .push_bind(pattern.clone())
            .push(" ESCAPE '!' OR u.real_name LIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '!')");
    }

    if let Some(role) = filter.role {
        qb.push(
            " AND EXISTS (SELECT 1 FROM user_roles ur_f
                          WHERE ur_f.user_id = u.id AND ur_f.role = ",
        )
        .push_bind(role.to_owned())
        .push(")");
    }

    if let Some(status) = filter.status {
        qb.push(" AND u.status = ").push_bind(status.to_owned());
    }
}

/// List users for the admin management screen, newest first.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn list_users_for_admin(
    pool: &MySqlPool,
    filter: &AdminUserFilter<'_>,
) -> Result<Vec<AdminUserRow>, DbError> {
    let mut qb = QueryBuilder::new(
        "SELECT u.id, u.username, u.email, u.real_name, u.status,
                u.created_at, u.last_login_at,
                COALESCE((SELECT GROUP_CONCAT(ur.role ORDER BY ur.role)
                          FROM user_roles ur WHERE ur.user_id = u.id), '') AS roles
         FROM users u
         WHERE 1 = 1",
    );
    push_admin_user_filters(&mut qb, filter);
    qb.push(" ORDER BY u.created_at DESC, u.id DESC LIMIT ")
        .push_bind(filter.limit)
        .push(" OFFSET ")
        .push_bind(filter.offset);

    let rows = qb.build_query_as::<AdminUserRow>().fetch_all(pool).await?;
    Ok(rows)
}

/// Count users matching the admin list filters.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn count_users_for_admin(
    pool: &MySqlPool,
    filter: &AdminUserFilter<'_>,
) -> Result<i64, DbError> {
    let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM users u WHERE 1 = 1");
    push_admin_user_filters(&mut qb, filter);

    let count: i64 = qb.build_query_scalar::<i64>().fetch_one(pool).await?;
    Ok(count)
}

/// Count active accounts holding the `admin` role.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn count_active_admins(pool: &MySqlPool) -> Result<i64, DbError> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM user_roles ur
         JOIN users u ON u.id = ur.user_id
         WHERE ur.role = 'admin' AND u.status = 'active'",
    )
    .fetch_one(pool)
    .await?;

    Ok(count)
}

/// Grant a role to a user. Idempotent: granting an existing role is a no-op.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the insert fails.
pub async fn grant_user_role(pool: &MySqlPool, user_id: u64, role: &str) -> Result<bool, DbError> {
    let res = sqlx::query(
        "INSERT IGNORE INTO user_roles (user_id, role, granted_at)
         VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(role)
    .bind(Utc::now())
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Revoke a role from a user, refusing to remove the last active administrator.
///
/// The target user row and the active administrator rows are locked inside one
/// transaction so two concurrent demotions cannot both observe "two admins".
///
/// # Errors
/// Returns [`DbError::Conflict`] if the user does not exist or the role is the
/// last active admin role, or [`DbError::Sqlx`] on query failure.
pub async fn revoke_user_role_guarded(
    pool: &MySqlPool,
    user_id: u64,
    role: &str,
) -> Result<bool, DbError> {
    let mut tx = pool.begin().await?;

    let target: Option<(u64, String)> =
        sqlx::query_as("SELECT id, status FROM users WHERE id = ? FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;

    let Some((_, _status)) = target else {
        tx.rollback().await?;
        return Err(DbError::Conflict("người dùng không tồn tại".to_string()));
    };

    if role == "admin" {
        let active_admins: Vec<(u64,)> = sqlx::query_as(
            "SELECT ur.user_id
             FROM user_roles ur
             JOIN users u ON u.id = ur.user_id
             WHERE ur.role = 'admin' AND u.status = 'active'
             FOR UPDATE",
        )
        .fetch_all(&mut *tx)
        .await?;

        let target_is_admin = active_admins.iter().any(|(id,)| *id == user_id);
        if target_is_admin && active_admins.len() <= 1 {
            tx.rollback().await?;
            return Err(DbError::Conflict(
                "Không thể thu hồi quyền quản trị viên cuối cùng.".to_string(),
            ));
        }
    }

    let res = sqlx::query("DELETE FROM user_roles WHERE user_id = ? AND role = ?")
        .bind(user_id)
        .bind(role)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

/// Change a user's account status, refusing to disable the last active admin.
///
/// # Errors
/// Returns [`DbError::Conflict`] if the user does not exist or the change would
/// disable the last active administrator, or [`DbError::Sqlx`] on query failure.
pub async fn set_user_status_guarded(
    pool: &MySqlPool,
    user_id: u64,
    status: &str,
) -> Result<bool, DbError> {
    let mut tx = pool.begin().await?;

    let target: Option<(u64, String)> =
        sqlx::query_as("SELECT id, status FROM users WHERE id = ? FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;

    let Some((_, current_status)) = target else {
        tx.rollback().await?;
        return Err(DbError::Conflict("người dùng không tồn tại".to_string()));
    };

    if status == "disabled" && current_status == "active" {
        let target_is_admin: Option<(u64,)> =
            sqlx::query_as("SELECT user_id FROM user_roles WHERE user_id = ? AND role = 'admin'")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;

        if target_is_admin.is_some() {
            let active_admins: Vec<(u64,)> = sqlx::query_as(
                "SELECT ur.user_id
                 FROM user_roles ur
                 JOIN users u ON u.id = ur.user_id
                 WHERE ur.role = 'admin' AND u.status = 'active'
                 FOR UPDATE",
            )
            .fetch_all(&mut *tx)
            .await?;

            if active_admins.len() <= 1 {
                tx.rollback().await?;
                return Err(DbError::Conflict(
                    "Không thể vô hiệu hóa quản trị viên cuối cùng.".to_string(),
                ));
            }
        }
    }

    let res = sqlx::query("UPDATE users SET status = ? WHERE id = ?")
        .bind(status)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
