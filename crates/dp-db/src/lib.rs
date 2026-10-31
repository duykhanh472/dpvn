//! Database access layer, connection pooling, and migrations.

pub mod models;
pub mod queries;

pub use queries::sessions::SessionUser;

use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use std::time::Duration;

/// Errors originating from database operations and migrations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Failure from the underlying sqlx driver.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Failure executing database migrations.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// Business or concurrency conflict (e.g. race on claiming or saving a page).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Domain error propagated during state transition evaluation.
    #[error("domain error: {0}")]
    Domain(#[from] dp_core::DomainError),
}

/// Create a new MySQL connection pool configured with the provided limits.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the pool cannot be initialized or the URL is malformed.
pub async fn create_pool(
    database_url: &str,
    min_connections: u32,
    max_connections: u32,
) -> Result<MySqlPool, DbError> {
    let pool = MySqlPoolOptions::new()
        .min_connections(min_connections)
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(600))
        .connect(database_url)
        .await?;

    Ok(pool)
}

/// Apply all embedded database migrations.
///
/// # Errors
/// Returns [`DbError::Migrate`] if migration execution fails.
pub async fn run_migrations(pool: &MySqlPool) -> Result<(), DbError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Ping the database to verify connectivity.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the ping query fails or times out.
pub async fn ping(pool: &MySqlPool) -> Result<(), DbError> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

/// Verify that the database is connected and initial schema migrations have been applied.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if querying fails.
pub async fn check_ready(pool: &MySqlPool) -> Result<bool, DbError> {
    // Check if the settings table created in 0001_init.sql exists and is queryable
    let res = sqlx::query("SELECT COUNT(*) FROM settings")
        .execute(pool)
        .await;

    match res {
        Ok(_) => Ok(true),
        Err(sqlx::Error::Database(err)) if err.message().contains("doesn't exist") => Ok(false),
        Err(e) => Err(DbError::Sqlx(e)),
    }
}
