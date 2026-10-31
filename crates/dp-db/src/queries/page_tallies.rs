//! User proofing tallies and daily limit enforcement in `Asia/Ho_Chi_Minh`.

use chrono::{FixedOffset, NaiveDate, Utc};
use sqlx::MySqlPool;

use crate::DbError;

/// Return today's calendar date in the `Asia/Ho_Chi_Minh` timezone (UTC+7).
#[must_use]
pub fn current_vietnam_date() -> NaiveDate {
    // Vietnam is fixed at UTC+7 with no daylight saving time shifts.
    let offset =
        FixedOffset::east_opt(7 * 3600).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    Utc::now().with_timezone(&offset).date_naive()
}

/// Retrieve the number of pages proofed by `user_id` in `round` for the specified `day`.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database query fails.
pub async fn get_daily_tally(
    pool: &MySqlPool,
    user_id: u64,
    round: &str,
    day: NaiveDate,
) -> Result<u32, DbError> {
    let row: (i64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(count), 0) AS SIGNED)
         FROM page_tallies
         WHERE user_id = ? AND round = ? AND day = ?",
    )
    .bind(user_id)
    .bind(round)
    .bind(day)
    .fetch_one(pool)
    .await?;

    Ok(row.0 as u32)
}

/// Retrieve the total number of pages proofed by `user_id` across all rounds on `day`.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_total_daily_tally(
    pool: &MySqlPool,
    user_id: u64,
    day: NaiveDate,
) -> Result<u32, DbError> {
    let row: (i64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(count), 0) AS SIGNED)
         FROM page_tallies
         WHERE user_id = ? AND day = ?",
    )
    .bind(user_id)
    .bind(day)
    .fetch_one(pool)
    .await?;

    Ok(row.0 as u32)
}

/// Retrieve the lifetime number of completed pages proofed by `user_id`.
///
/// Used to gate access to rounds configured with `min_pages`.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database query fails.
pub async fn get_lifetime_tally(pool: &MySqlPool, user_id: u64) -> Result<u32, DbError> {
    let row: (i64,) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(count), 0) AS SIGNED)
         FROM page_tallies
         WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    Ok(row.0 as u32)
}

/// Check whether the user has exceeded the daily proofing limit.
///
/// Returns `Ok(true)` if the user is allowed to claim another page, or `Err(DomainError::DailyLimitReached)`
/// if they have reached or exceeded `daily_limit`.
///
/// # Errors
/// Returns [`DbError::Domain`] with [`dp_core::DomainError::DailyLimitReached`] if over limit,
/// or [`DbError::Sqlx`] on DB failure.
pub async fn check_daily_limit(
    pool: &MySqlPool,
    user_id: u64,
    daily_limit: u32,
) -> Result<(), DbError> {
    let today = current_vietnam_date();
    let current_count = get_total_daily_tally(pool, user_id, today).await?;

    if current_count >= daily_limit {
        return Err(DbError::Domain(dp_core::DomainError::DailyLimitReached {
            limit: daily_limit,
        }));
    }

    Ok(())
}
