//! Concurrency-safe claiming, saving, and auto-release of page rounds.
//!
//! Follows Section 6 of the implementation plan:
//! - Section 6.2: Candidate picking + conditional atomic claim
//! - Section 6.3: Critical section save with `FOR UPDATE` lock serialization
//! - Section 6.5: Idempotent round advance seeding
//! - Section 6.6: Auto-release of stale claims

use chrono::{DateTime, Utc};
use dp_core::RoundId;
use dp_core::round::RoundsConfig;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};

use super::page_tallies::current_vietnam_date;
use crate::DbError;

/// Details of a claimed page ready for proofing.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ClaimedPage {
    /// Page ID.
    pub page_id: u64,
    /// Parent project ID.
    pub project_id: u64,
    /// Sequential page number.
    pub page_number: i32,
    /// Active round identifier.
    pub round: String,
    /// Path to image scan.
    pub image_path: String,
    /// Image width in pixels.
    pub image_width: u32,
    /// Image height in pixels.
    pub image_height: u32,
    /// Text carried over into this round.
    pub text: Option<String>,
    /// Immutable OCR text baseline.
    pub ocr_text: Option<String>,
    /// Timestamp when claim was registered.
    pub claimed_at: DateTime<Utc>,
}

/// DTO for saving page text in the critical section.
#[derive(Debug, Clone)]
pub struct SavePageInput<'a> {
    /// Parent project ID.
    pub project_id: u64,
    /// Target page ID.
    pub page_id: u64,
    /// Active round.
    pub round: &'a str,
    /// Proofreader user ID.
    pub user_id: u64,
    /// Final proofed text.
    pub text: &'a str,
}

/// Outcome of saving a page, including any triggered round or phase advance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePageOutcome {
    /// Page ID saved.
    pub page_id: u64,
    /// Active round saved.
    pub round: String,
    /// Next round seeded if round completed, or None.
    pub advanced_round: Option<String>,
    /// New project phase if transitioned (e.g. "post_processing"), or None.
    pub transitioned_phase: Option<String>,
}

/// Candidate row fetched during claim step 1.
#[derive(Debug, FromRow)]
struct CandidatePage {
    id: u64,
}

// ───────────────────────────────────────────────────────────────────────────────
// Section 6.2: Claiming a page
// ───────────────────────────────────────────────────────────────────────────────

/// Pick an available candidate page and atomically claim it.
///
/// Executes the two-step dance specified in Section 6.2:
/// 1. Candidate selection with strict own-page exclusion from `prev_round`.
/// 2. Atomic conditional `UPDATE` where the `WHERE state = 'available'` is the lock.
///
/// If another transaction claims the same candidate, it retries up to `max_retries`
/// times before returning [`DbError::Conflict`].
///
/// Returns `Ok(None)` if no eligible available pages exist.
///
/// # Errors
/// Returns [`DbError::Conflict`] if retries are exhausted under high contention,
/// or [`DbError::Sqlx`] on DB failure.
pub async fn claim_next_available_page(
    pool: &MySqlPool,
    project_id: u64,
    round: &str,
    prev_round: Option<&str>,
    user_id: u64,
    max_retries: usize,
) -> Result<Option<ClaimedPage>, DbError> {
    for _ in 0..=max_retries {
        // Step 1: Pick a candidate (read-only, no lock)
        let candidate: Option<CandidatePage> = sqlx::query_as(
            "SELECT p.id
             FROM page_rounds pr
             JOIN pages p      ON p.id = pr.page_id
             JOIN projects pj  ON pj.id = p.project_id
             WHERE pj.id = ?
               AND pr.round = ?
               AND pr.state = 'available'
               AND pj.phase = 'in_round'
               AND pj.current_round = ?
               AND pj.on_hold = 0
               AND (? IS NULL OR NOT EXISTS (
                     SELECT 1 FROM page_rounds prev
                     WHERE prev.page_id = pr.page_id
                       AND prev.round = ?
                       AND prev.proofer_id = ?))
             ORDER BY p.page_number
             LIMIT 1",
        )
        .bind(project_id)
        .bind(round)
        .bind(round)
        .bind(prev_round)
        .bind(prev_round)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

        let Some(cand) = candidate else {
            // No available pages matching criteria
            return Ok(None);
        };

        // Step 2: Claim atomically; the WHERE clause is the lock
        let now = Utc::now();
        let res = sqlx::query(
            "UPDATE page_rounds
                SET state = 'in_progress',
                    proofer_id = ?,
                    claimed_at = ?
              WHERE page_id = ?
                AND round = ?
                AND state = 'available'",
        )
        .bind(user_id)
        .bind(now)
        .bind(cand.id)
        .bind(round)
        .execute(pool)
        .await?;

        if res.rows_affected() == 1 {
            // Successfully claimed! Fetch full page context to return
            let claimed = sqlx::query_as::<_, ClaimedPage>(
                "SELECT p.id AS page_id, p.project_id, p.page_number, pr.round,
                        p.image_path, p.image_width, p.image_height, pr.text,
                        p.ocr_text, pr.claimed_at
                 FROM page_rounds pr
                 JOIN pages p ON p.id = pr.page_id
                 WHERE pr.page_id = ? AND pr.round = ?",
            )
            .bind(cand.id)
            .bind(round)
            .fetch_one(pool)
            .await?;

            return Ok(Some(claimed));
        }

        // rows_affected == 0: someone else claimed it first, retry
        tracing::debug!(
            page_id = cand.id,
            "claim collision, retrying candidate query"
        );
    }

    Err(DbError::Conflict(
        "không thể nhận trang do xung đột đồng thời cao, vui lòng thử lại".to_string(),
    ))
}

/// Release a claimed page back to 'available' state voluntarily by the proofer.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database update fails.
pub async fn release_claim(
    pool: &MySqlPool,
    page_id: u64,
    round: &str,
    user_id: u64,
) -> Result<bool, DbError> {
    let res = sqlx::query(
        "UPDATE page_rounds
            SET state = 'available', proofer_id = NULL, claimed_at = NULL
          WHERE page_id = ?
            AND round = ?
            AND proofer_id = ?
            AND state = 'in_progress'",
    )
    .bind(page_id)
    .bind(round)
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

// ───────────────────────────────────────────────────────────────────────────────
// Section 6.3: Saving a page — the critical section
// ───────────────────────────────────────────────────────────────────────────────

/// Save page text, create version snapshot, update tallies, and evaluate round advancement.
///
/// Implements the critical section from Section 6.3:
/// 1. Transaction begins.
/// 2. `SELECT ... FOR UPDATE` locks the project row (only serialization point).
/// 3. Re-checks project phase and current round.
/// 4. Updates `page_rounds` verifying `state = 'in_progress'` and `proofer_id = user_id`.
/// 5. Inserts historical `page_versions` snapshot.
/// 6. Increments `page_tallies` for today in `Asia/Ho_Chi_Minh`.
/// 7. Checks remaining incomplete pages in round.
/// 8. If 0 remaining: advances round or transitions project to `post_processing`,
///    idempotently seeds next round rows, and appends `project_events` audit record.
/// 9. Transaction commits.
///
/// # Errors
/// Returns [`DbError::Conflict`] if the page was not held by the user or already saved,
/// or [`DbError::Sqlx`] on DB failure.
pub async fn save_page(
    pool: &MySqlPool,
    input: SavePageInput<'_>,
    rounds_config: &RoundsConfig,
) -> Result<SavePageOutcome, DbError> {
    let mut tx = pool.begin().await?;

    // 1. Lock the project first (Global lock ordering: projects -> pages -> page_rounds)
    let project_row: Option<(u64, String, Option<String>, i8)> = sqlx::query_as(
        "SELECT id, phase, current_round, on_hold
         FROM projects
         WHERE id = ?
         FOR UPDATE",
    )
    .bind(input.project_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((_pid, phase, current_round_opt, on_hold)) = project_row else {
        tx.rollback().await?;
        return Err(DbError::Conflict("dự án không tồn tại".to_string()));
    };

    if on_hold != 0 {
        tx.rollback().await?;
        return Err(DbError::Conflict(
            "dự án đang tạm dừng (on_hold = 1), không thể lưu trang".to_string(),
        ));
    }

    if phase != "in_round" {
        tx.rollback().await?;
        return Err(DbError::Conflict(format!(
            "dự án không ở trạng thái in_round (trạng thái hiện tại: {phase})"
        )));
    }

    if current_round_opt.as_deref() != Some(input.round) {
        tx.rollback().await?;
        return Err(DbError::Conflict(format!(
            "vòng hoạt động của dự án ({:?}) không khớp với vòng trang đang lưu ({})",
            current_round_opt, input.round
        )));
    }

    // 2. Verify the page is ours and still in progress
    let now = Utc::now();
    let res = sqlx::query(
        "UPDATE page_rounds
            SET state = 'done',
                text = ?,
                done_at = ?
          WHERE page_id = ?
            AND round = ?
            AND state = 'in_progress'
            AND proofer_id = ?",
    )
    .bind(input.text)
    .bind(now)
    .bind(input.page_id)
    .bind(input.round)
    .bind(input.user_id)
    .execute(&mut *tx)
    .await?;

    if res.rows_affected() != 1 {
        tx.rollback().await?;
        return Err(DbError::Conflict(
            "trang không ở trạng thái in_progress hoặc bạn không phải người giữ trang".to_string(),
        ));
    }

    // 3. Record historical version
    sqlx::query(
        "INSERT INTO page_versions (page_id, round, text, proofer_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(input.page_id)
    .bind(input.round)
    .bind(input.text)
    .bind(input.user_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // 4. Update daily page tally in Asia/Ho_Chi_Minh
    let local_day = current_vietnam_date();
    sqlx::query(
        "INSERT INTO page_tallies (user_id, round, day, count)
         VALUES (?, ?, ?, 1)
         ON DUPLICATE KEY UPDATE count = count + 1",
    )
    .bind(input.user_id)
    .bind(input.round)
    .bind(local_day)
    .execute(&mut *tx)
    .await?;

    // 5. Are we the last page in this round?
    let incomplete_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM page_rounds
         WHERE round = ?
           AND state <> 'done'
           AND page_id IN (SELECT id FROM pages WHERE project_id = ?)",
    )
    .bind(input.round)
    .bind(input.project_id)
    .fetch_one(&mut *tx)
    .await?;

    let mut outcome = SavePageOutcome {
        page_id: input.page_id,
        round: input.round.to_string(),
        advanced_round: None,
        transitioned_phase: None,
    };

    if incomplete_count.0 == 0 {
        let current_rid =
            RoundId::new(input.round).map_err(|e| DbError::Conflict(e.to_string()))?;

        if rounds_config.is_last_round(&current_rid) {
            // Final round completed: transition to PostProcessing
            sqlx::query(
                "UPDATE projects
                 SET phase = 'post_processing',
                     current_round = NULL,
                     updated_at = ?
                 WHERE id = ?",
            )
            .bind(now)
            .bind(input.project_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "INSERT INTO project_events (
                    project_id, from_phase, to_phase, from_round, to_round,
                    actor_id, note, created_at
                 ) VALUES (?, 'in_round', 'post_processing', ?, NULL, NULL, ?, ?)",
            )
            .bind(input.project_id)
            .bind(input.round)
            .bind(format!(
                "hoàn thành vòng cuối {}, chuyển sang hậu kỳ",
                input.round
            ))
            .bind(now)
            .execute(&mut *tx)
            .await?;

            outcome.transitioned_phase = Some("post_processing".to_string());
        } else if let Some(next_round_cfg) = rounds_config.next_round(&current_rid) {
            let next_rid_str = next_round_cfg.id.as_str();

            // Advance project current_round
            sqlx::query(
                "UPDATE projects
                 SET current_round = ?,
                     updated_at = ?
                 WHERE id = ?",
            )
            .bind(next_rid_str)
            .bind(now)
            .bind(input.project_id)
            .execute(&mut *tx)
            .await?;

            // Seed next round rows idempotently (INSERT IGNORE treats already seeded as success)
            sqlx::query(
                "INSERT IGNORE INTO page_rounds (page_id, round, state, text)
                 SELECT p.id, ?, 'available', pr.text
                   FROM pages p
                   JOIN page_rounds pr
                     ON pr.page_id = p.id
                    AND pr.round = ?
                  WHERE p.project_id = ?",
            )
            .bind(next_rid_str)
            .bind(input.round)
            .bind(input.project_id)
            .execute(&mut *tx)
            .await?;

            // Append audit event
            sqlx::query(
                "INSERT INTO project_events (
                    project_id, from_phase, to_phase, from_round, to_round,
                    actor_id, note, created_at
                 ) VALUES (?, 'in_round', 'in_round', ?, ?, NULL, ?, ?)",
            )
            .bind(input.project_id)
            .bind(input.round)
            .bind(next_rid_str)
            .bind(format!(
                "hoàn thành vòng {}, chuyển sang vòng {}",
                input.round, next_rid_str
            ))
            .bind(now)
            .execute(&mut *tx)
            .await?;

            outcome.advanced_round = Some(next_rid_str.to_string());
        }
    }

    tx.commit().await?;

    Ok(outcome)
}

// ───────────────────────────────────────────────────────────────────────────────
// Section 6.6: Auto-release of stale claims
// ───────────────────────────────────────────────────────────────────────────────

/// Release in-progress claims that have exceeded the timeout window and have not saved draft text.
///
/// Returns the number of claims released.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database update fails.
pub async fn release_stale_claims(pool: &MySqlPool, timeout_hours: u32) -> Result<u64, DbError> {
    let res = sqlx::query(
        "UPDATE page_rounds
            SET state = 'available', proofer_id = NULL, claimed_at = NULL
          WHERE state = 'in_progress'
            AND claimed_at < UTC_TIMESTAMP(6) - INTERVAL ? HOUR
            AND text IS NULL",
    )
    .bind(timeout_hours)
    .execute(pool)
    .await?;

    Ok(res.rows_affected())
}

// ───────────────────────────────────────────────────────────────────────────────
// Inspection & User Queries
// ───────────────────────────────────────────────────────────────────────────────

/// Retrieve currently active claim for a user, if any.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_user_active_claim(
    pool: &MySqlPool,
    user_id: u64,
) -> Result<Option<ClaimedPage>, DbError> {
    let row = sqlx::query_as::<_, ClaimedPage>(
        "SELECT p.id AS page_id, p.project_id, p.page_number, pr.round,
                p.image_path, p.image_width, p.image_height, pr.text,
                p.ocr_text, pr.claimed_at
         FROM page_rounds pr
         JOIN pages p ON p.id = pr.page_id
         WHERE pr.proofer_id = ?
           AND pr.state = 'in_progress'
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Retrieve round data for a specific page.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_page_round(
    pool: &MySqlPool,
    page_id: u64,
    round: &str,
) -> Result<Option<crate::models::PageRound>, DbError> {
    let row = sqlx::query_as::<_, crate::models::PageRound>(
        "SELECT page_id, round, state, text, proofer_id, claimed_at, done_at
         FROM page_rounds
         WHERE page_id = ? AND round = ?",
    )
    .bind(page_id)
    .bind(round)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Summary of a page's status in a round for project detail views.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PageRoundSummary {
    /// Page sequence number.
    pub page_number: i32,
    /// Page ID.
    pub page_id: u64,
    /// Round name.
    pub round: String,
    /// Page state.
    pub state: String,
    /// Proofer username if claimed or completed.
    pub proofer_username: Option<String>,
    /// Done timestamp if completed.
    pub done_at: Option<DateTime<Utc>>,
}

/// List all page states for a specific project and round.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn list_project_page_rounds(
    pool: &MySqlPool,
    project_id: u64,
    round: &str,
) -> Result<Vec<PageRoundSummary>, DbError> {
    let rows = sqlx::query_as::<_, PageRoundSummary>(
        "SELECT p.page_number, p.id AS page_id, pr.round, pr.state,
                u.username AS proofer_username, pr.done_at
         FROM pages p
         JOIN page_rounds pr ON pr.page_id = p.id AND pr.round = ?
         LEFT JOIN users u ON u.id = pr.proofer_id
         WHERE p.project_id = ?
         ORDER BY p.page_number ASC",
    )
    .bind(round)
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
