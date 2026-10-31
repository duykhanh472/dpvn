//! Database queries for project records, metadata, and phase updates.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use dp_core::{
    ActorRole, Effect, Phase, ProjectContext, ProjectId, RoundId, UserId, apply,
    round::RoundsConfig, toggle_on_hold,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySql, MySqlPool, QueryBuilder, Transaction};

use crate::DbError;
use crate::models::Project;

/// DTO for creating a new project.
#[derive(Debug, Clone)]
pub struct NewProject<'a> {
    /// Title / name of the book or document.
    pub name: &'a str,
    /// Book author.
    pub author: &'a str,
    /// Tone mark style ('old', 'new', 'either').
    pub tone_style: &'a str,
    /// Character set definition key (e.g., 'vi').
    pub charset: &'a str,
    /// ID of the user managing this project.
    pub manager_id: u64,
    /// Image source attribution or provenance.
    pub image_source: &'a str,
    /// Initial project instructions or notes.
    pub comments: &'a str,
}

/// Project summary item enriched with manager name and page count for lists.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectListItem {
    /// Project ID.
    pub id: u64,
    /// Title / name.
    pub name: String,
    /// Author.
    pub author: String,
    /// Current lifecycle phase ('draft', 'queued', 'in_round', etc.).
    pub phase: String,
    /// Active round if in 'in_round' phase.
    pub current_round: Option<String>,
    /// Whether project is on hold (0 = active, 1 = hold).
    pub on_hold: i8,
    /// Tone mark style convention.
    pub tone_style: String,
    /// Character set identifier.
    pub charset: String,
    /// Manager user ID.
    pub manager_id: u64,
    /// Manager username.
    pub manager_username: String,
    /// Attribution/source of page scans.
    pub image_source: String,
    /// Project manager instructions / comments.
    pub comments: String,
    /// Total number of pages imported into this project.
    pub total_pages: i64,
    /// Unclaimed pages in the active round.
    pub available_pages: i64,
    /// Pages currently claimed in the active round.
    pub in_progress_pages: i64,
    /// Pages completed in the active round.
    pub done_pages: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Create a new project in 'draft' phase.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database insertion fails.
pub async fn create_project(pool: &MySqlPool, input: &NewProject<'_>) -> Result<u64, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO projects (
            name, author, phase, current_round, on_hold,
            tone_style, charset, manager_id, image_source, comments,
            created_at, updated_at
        ) VALUES (?, ?, 'draft', NULL, 0, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(input.name)
    .bind(input.author)
    .bind(input.tone_style)
    .bind(input.charset)
    .bind(input.manager_id)
    .bind(input.image_source)
    .bind(input.comments)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(res.last_insert_id())
}

/// Find a project by its primary key ID.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database query fails.
pub async fn get_project_by_id(pool: &MySqlPool, id: u64) -> Result<Option<Project>, DbError> {
    let row = sqlx::query_as::<_, Project>(
        "SELECT id, name, author, phase, current_round, on_hold,
                tone_style, charset, manager_id, image_source, comments,
                created_at, updated_at
         FROM projects
         WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Retrieve project with manager information and page count.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_project_detail(
    pool: &MySqlPool,
    id: u64,
) -> Result<Option<ProjectListItem>, DbError> {
    let row = sqlx::query_as::<_, ProjectListItem>(
        "SELECT p.id, p.name, p.author, p.phase, p.current_round, p.on_hold,
                p.tone_style, p.charset, p.manager_id, u.username AS manager_username,
                p.image_source, p.comments,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id), 0) AS total_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state = 'available'), 0) AS available_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state = 'in_progress'), 0) AS in_progress_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state = 'done'), 0) AS done_pages,
                p.created_at, p.updated_at
         FROM projects p
         JOIN users u ON u.id = p.manager_id
         WHERE p.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Filter and sort options for the project list (§13.1, Phase 8).
#[derive(Debug, Clone, Default)]
pub struct ProjectListFilter<'a> {
    /// Restrict to projects managed by this user ("my projects").
    pub manager_id: Option<u64>,
    /// Restrict to projects whose current round matches.
    pub round: Option<&'a str>,
    /// `available`, `in_progress`, `complete`, or `finished`.
    pub availability: Option<&'a str>,
    /// Case/accent-sensitive substring match on name or author.
    pub search: Option<&'a str>,
    /// Ordering key (`recent`, `created`, `name`, `pages`, `pages_asc`).
    pub sort: &'a str,
    /// Page size, already capped by the caller.
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

/// Append the shared `WHERE` predicates for [`list_projects_filtered`] and
/// [`count_projects_filtered`].
fn push_project_filters(qb: &mut QueryBuilder<'_, MySql>, filter: &ProjectListFilter<'_>) {
    if let Some(manager_id) = filter.manager_id {
        qb.push(" AND p.manager_id = ").push_bind(manager_id);
    }

    if let Some(round) = filter.round {
        qb.push(" AND p.current_round = ")
            .push_bind(round.to_owned());
    }

    match filter.availability {
        Some("available") => {
            qb.push(
                " AND p.phase = 'in_round' AND p.on_hold = 0
                  AND EXISTS (
                    SELECT 1 FROM page_rounds pr_av
                    JOIN pages pg_av ON pg_av.id = pr_av.page_id
                    WHERE pg_av.project_id = p.id
                      AND pr_av.round = p.current_round
                      AND pr_av.state = 'available'
                  )",
            );
        }
        Some("in_progress") => {
            qb.push(
                " AND p.phase = 'in_round'
                  AND EXISTS (
                    SELECT 1 FROM page_rounds pr_ip
                    JOIN pages pg_ip ON pg_ip.id = pr_ip.page_id
                    WHERE pg_ip.project_id = p.id
                      AND pr_ip.round = p.current_round
                      AND pr_ip.state = 'in_progress'
                  )",
            );
        }
        Some("complete") => {
            qb.push(
                " AND p.phase = 'in_round'
                  AND EXISTS (
                    SELECT 1 FROM page_rounds pr_cp
                    JOIN pages pg_cp ON pg_cp.id = pr_cp.page_id
                    WHERE pg_cp.project_id = p.id
                      AND pr_cp.round = p.current_round
                      AND pr_cp.state = 'done'
                  )
                  AND NOT EXISTS (
                    SELECT 1 FROM page_rounds pr_cp
                    JOIN pages pg_cp ON pg_cp.id = pr_cp.page_id
                    WHERE pg_cp.project_id = p.id
                      AND pr_cp.round = p.current_round
                      AND pr_cp.state <> 'done'
                  )",
            );
        }
        Some("finished") => {
            qb.push(" AND p.phase IN ('post_processing', 'posted', 'archived')");
        }
        _ => {}
    }

    if let Some(search) = filter.search {
        let pattern = like_pattern(search);
        qb.push(" AND (p.name LIKE ")
            .push_bind(pattern.clone())
            .push(" ESCAPE '!' OR p.author LIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '!')");
    }
}

/// Map a user-supplied sort key to a static `ORDER BY` clause.
///
/// Only whitelisted names are interpolated, so the value can never inject SQL.
fn project_order_clause(sort: &str) -> &'static str {
    match sort {
        "created" => "p.created_at DESC, p.id DESC",
        "name" => "p.name ASC, p.id ASC",
        "pages" => "total_pages DESC, p.name ASC",
        "pages_asc" => "total_pages ASC, p.name ASC",
        _ => "p.updated_at DESC, p.id DESC",
    }
}

/// List projects matching `filter`, ordered and paginated.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn list_projects_filtered(
    pool: &MySqlPool,
    filter: &ProjectListFilter<'_>,
) -> Result<Vec<ProjectListItem>, DbError> {
    let mut qb = QueryBuilder::new(
        "SELECT p.id, p.name, p.author, p.phase, p.current_round, p.on_hold,
                p.tone_style, p.charset, p.manager_id, u.username AS manager_username,
                p.image_source, p.comments,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id), 0) AS total_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state = 'available'), 0) AS available_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state = 'in_progress'), 0) AS in_progress_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state = 'done'), 0) AS done_pages,
                p.created_at, p.updated_at
         FROM projects p
         JOIN users u ON u.id = p.manager_id
         WHERE 1 = 1",
    );
    push_project_filters(&mut qb, filter);
    qb.push(" ORDER BY ")
        .push(project_order_clause(filter.sort))
        .push(" LIMIT ")
        .push_bind(filter.limit)
        .push(" OFFSET ")
        .push_bind(filter.offset);

    let rows = qb
        .build_query_as::<ProjectListItem>()
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Count projects matching `filter`, ignoring pagination.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn count_projects_filtered(
    pool: &MySqlPool,
    filter: &ProjectListFilter<'_>,
) -> Result<i64, DbError> {
    let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM projects p WHERE 1 = 1");
    push_project_filters(&mut qb, filter);

    let count: i64 = qb.build_query_scalar::<i64>().fetch_one(pool).await?;
    Ok(count)
}

/// Completion progress for a project's active round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectProgress {
    /// Total pages in the project.
    pub total_pages: i64,
    /// Unclaimed pages in the active round.
    pub available_pages: i64,
    /// Claimed pages in the active round.
    pub in_progress_pages: i64,
    /// Completed pages in the active round.
    pub done_pages: i64,
    /// Completion percentage, 0–100.
    pub percent: u32,
}

/// Compute round completion counts for a project.
///
/// When `round` is `None` (draft or between rounds), the round counts are zero
/// while `total_pages` still reflects the imported page count.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if the query fails.
pub async fn get_project_progress(
    pool: &MySqlPool,
    project_id: u64,
    round: Option<&str>,
) -> Result<ProjectProgress, DbError> {
    #[derive(FromRow)]
    struct RawProgress {
        total_pages: i64,
        available_pages: i64,
        in_progress_pages: i64,
        done_pages: i64,
    }

    let row = sqlx::query_as::<_, RawProgress>(
        "SELECT COUNT(*) AS total_pages,
                CAST(COALESCE(SUM(CASE WHEN pr.state = 'available' THEN 1 ELSE 0 END), 0) AS SIGNED) AS available_pages,
                CAST(COALESCE(SUM(CASE WHEN pr.state = 'in_progress' THEN 1 ELSE 0 END), 0) AS SIGNED) AS in_progress_pages,
                CAST(COALESCE(SUM(CASE WHEN pr.state = 'done' THEN 1 ELSE 0 END), 0) AS SIGNED) AS done_pages
         FROM pages p
         LEFT JOIN page_rounds pr
                ON pr.page_id = p.id
               AND pr.round = ?
         WHERE p.project_id = ?",
    )
    .bind(round.unwrap_or(""))
    .bind(project_id)
    .fetch_one(pool)
    .await?;

    let percent = if row.total_pages > 0 {
        ((row.done_pages.max(0) as u64 * 100) / row.total_pages as u64) as u32
    } else {
        0
    };

    Ok(ProjectProgress {
        total_pages: row.total_pages,
        available_pages: row.available_pages,
        in_progress_pages: row.in_progress_pages,
        done_pages: row.done_pages,
        percent,
    })
}

/// DTO for updating project metadata fields.
#[derive(Debug, Clone)]
pub struct UpdateProjectMetadata<'a> {
    /// Title / name of the book or document.
    pub name: &'a str,
    /// Book author.
    pub author: &'a str,
    /// Tone mark style ('old', 'new', 'either').
    pub tone_style: &'a str,
    /// Character set definition key (e.g., 'vi').
    pub charset: &'a str,
    /// Image source attribution or provenance.
    pub image_source: &'a str,
    /// Project instructions or notes.
    pub comments: &'a str,
}

/// Update project metadata fields.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if update fails.
pub async fn update_project_metadata(
    pool: &MySqlPool,
    id: u64,
    input: &UpdateProjectMetadata<'_>,
) -> Result<bool, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "UPDATE projects
         SET name = ?, author = ?, tone_style = ?, charset = ?,
             image_source = ?, comments = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(input.name)
    .bind(input.author)
    .bind(input.tone_style)
    .bind(input.charset)
    .bind(input.image_source)
    .bind(input.comments)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Update project phase and current round.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if update query fails.
pub async fn update_project_phase(
    pool: &MySqlPool,
    id: u64,
    phase: &str,
    current_round: Option<&str>,
) -> Result<bool, DbError> {
    let now = Utc::now();
    let res = sqlx::query(
        "UPDATE projects
         SET phase = ?, current_round = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(phase)
    .bind(current_round)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}

/// Delete all pages associated with a project.
///
/// Due to ON DELETE CASCADE foreign keys, related `page_rounds` and `page_versions`
/// are cleaned up automatically.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if deletion fails.
pub async fn delete_project_pages(pool: &MySqlPool, project_id: u64) -> Result<u64, DbError> {
    let res = sqlx::query("DELETE FROM pages WHERE project_id = ?")
        .bind(project_id)
        .execute(pool)
        .await?;

    Ok(res.rows_affected())
}

/// Project pool item displayed to proofreaders searching for available work.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectPoolItem {
    /// Project ID.
    pub id: u64,
    /// Book title.
    pub name: String,
    /// Book author.
    pub author: String,
    /// Active round.
    pub current_round: Option<String>,
    /// Tone style convention.
    pub tone_style: String,
    /// Manager username.
    pub manager_username: String,
    /// Total pages in book.
    pub total_pages: i64,
    /// Available unclaimed pages remaining in this round.
    pub available_pages: i64,
}

/// List active projects in `round` that have work available and are not paused.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn list_available_projects_in_round(
    pool: &MySqlPool,
    round: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<ProjectPoolItem>, DbError> {
    let rows = sqlx::query_as::<_, ProjectPoolItem>(
        "SELECT p.id, p.name, p.author, p.current_round, p.tone_style,
                u.username AS manager_username,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id), 0) AS total_pages,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id AND pr.round = ? AND pr.state = 'available'), 0) AS available_pages
         FROM projects p
         JOIN users u ON u.id = p.manager_id
         WHERE p.phase = 'in_round'
           AND p.current_round = ?
           AND p.on_hold = 0
         ORDER BY p.updated_at DESC
         LIMIT ? OFFSET ?",
    )
    .bind(round)
    .bind(round)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Fetch a [`ProjectContext`] snapshot used to evaluate pure domain state transitions.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if query fails.
pub async fn get_project_context(
    pool: &MySqlPool,
    project_id: u64,
) -> Result<Option<ProjectContext>, DbError> {
    #[derive(FromRow)]
    struct RawContext {
        id: u64,
        phase: String,
        current_round: Option<String>,
        on_hold: i8,
        total_pages: i64,
        pages_with_image: i64,
        pages_with_ocr: i64,
        round_incomplete_pages: i64,
    }

    let row = sqlx::query_as::<_, RawContext>(
        "SELECT p.id, p.phase, p.current_round, p.on_hold,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id), 0) AS total_pages,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id AND pg.image_path <> ''), 0) AS pages_with_image,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id AND pg.ocr_text IS NOT NULL AND pg.ocr_text <> ''), 0) AS pages_with_ocr,
                COALESCE((SELECT COUNT(*) FROM page_rounds pr
                          JOIN pages pg ON pg.id = pr.page_id
                          WHERE pg.project_id = p.id
                            AND pr.round = p.current_round
                            AND pr.state <> 'done'), 0) AS round_incomplete_pages
         FROM projects p
         WHERE p.id = ?",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    let phase = Phase::from_str(&r.phase)
        .map_err(|e| DbError::Conflict(format!("trạng thái phase không hợp lệ trong DB: {e}")))?;

    let current_round = match r.current_round {
        Some(s) if !s.is_empty() => Some(
            RoundId::new(s)
                .map_err(|e| DbError::Conflict(format!("round id không hợp lệ: {e}")))?,
        ),
        _ => None,
    };

    Ok(Some(ProjectContext {
        id: ProjectId::new(r.id),
        phase,
        current_round,
        on_hold: r.on_hold != 0,
        total_pages: r.total_pages as usize,
        pages_with_image: r.pages_with_image as usize,
        pages_with_ocr: r.pages_with_ocr as usize,
        round_incomplete_pages: r.round_incomplete_pages as usize,
    }))
}

/// Execute pure domain effects inside an active database transaction.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if any SQL statement fails.
pub async fn execute_effects_tx(
    tx: &mut Transaction<'_, MySql>,
    effects: &[Effect],
) -> Result<(), DbError> {
    let now = Utc::now();

    for effect in effects {
        match effect {
            Effect::UpdateProjectPhase {
                project_id,
                phase,
                current_round,
            } => {
                let round_str = current_round.as_ref().map(RoundId::as_str);
                sqlx::query(
                    "UPDATE projects
                     SET phase = ?, current_round = ?, updated_at = ?
                     WHERE id = ?",
                )
                .bind(phase.as_str())
                .bind(round_str)
                .bind(now)
                .bind(project_id.get())
                .execute(&mut **tx)
                .await?;
            }
            Effect::SeedRound {
                project_id,
                from_round,
                to_round,
            } => {
                sqlx::query(
                    "INSERT IGNORE INTO page_rounds (page_id, round, state, text)
                     SELECT p.id, ?, 'available', pr.text
                       FROM pages p
                       JOIN page_rounds pr
                         ON pr.page_id = p.id
                        AND pr.round = ?
                      WHERE p.project_id = ?",
                )
                .bind(to_round.as_str())
                .bind(from_round.as_str())
                .bind(project_id.get())
                .execute(&mut **tx)
                .await?;
            }
            Effect::RecordEvent {
                project_id,
                from_phase,
                to_phase,
                from_round,
                to_round,
                actor_id,
                note,
            } => {
                let fp = from_phase.map(|p| p.as_str());
                let tp = to_phase.map(|p| p.as_str());
                let fr = from_round.as_ref().map(RoundId::as_str);
                let tr = to_round.as_ref().map(RoundId::as_str);
                let aid = actor_id.map(|u| u.get());

                sqlx::query(
                    "INSERT INTO project_events (
                        project_id, from_phase, to_phase, from_round, to_round,
                        actor_id, note, created_at
                     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(project_id.get())
                .bind(fp)
                .bind(tp)
                .bind(fr)
                .bind(tr)
                .bind(aid)
                .bind(note)
                .bind(now)
                .execute(&mut **tx)
                .await?;
            }
            Effect::ToggleOnHold {
                project_id,
                on_hold,
            } => {
                sqlx::query(
                    "UPDATE projects
                     SET on_hold = ?, updated_at = ?
                     WHERE id = ?",
                )
                .bind(if *on_hold { 1i8 } else { 0i8 })
                .bind(now)
                .bind(project_id.get())
                .execute(&mut **tx)
                .await?;
            }
        }
    }

    Ok(())
}

/// Transition a project from its current phase to `target_phase` atomically.
///
/// Follows the strict lock ordering:
/// 1. Transaction begins.
/// 2. `SELECT ... FOR UPDATE` acquires the exclusive row lock on `projects`.
/// 3. Builds authoritative [`ProjectContext`].
/// 4. Calls pure `apply()` from `dp-core` to calculate effects.
/// 5. Executes effects via [`execute_effects_tx`].
/// 6. Transaction commits.
///
/// # Errors
/// Returns [`DbError::Conflict`] if project is not found or transition is denied,
/// or [`DbError::Sqlx`] on DB failure.
pub async fn transition_project(
    pool: &MySqlPool,
    project_id: u64,
    target_phase: Phase,
    actor_role: ActorRole,
    actor_id: Option<UserId>,
    rounds_config: &RoundsConfig,
) -> Result<Vec<Effect>, DbError> {
    let mut tx = pool.begin().await?;

    // Exclusive serialization lock
    let proj: Option<(u64, String, Option<String>, i8)> = sqlx::query_as(
        "SELECT id, phase, current_round, on_hold
         FROM projects
         WHERE id = ?
         FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((pid, phase_str, current_round_str, on_hold)) = proj else {
        tx.rollback().await?;
        return Err(DbError::Conflict("dự án không tồn tại".to_string()));
    };

    let phase = Phase::from_str(&phase_str)
        .map_err(|e| DbError::Conflict(format!("trạng thái phase không hợp lệ: {e}")))?;

    let current_round = match current_round_str {
        Some(s) if !s.is_empty() => Some(
            RoundId::new(s)
                .map_err(|e| DbError::Conflict(format!("round id không hợp lệ: {e}")))?,
        ),
        _ => None,
    };

    // Calculate counts
    let total_pages: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pages WHERE project_id = ?")
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;

    let pages_with_image: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pages WHERE project_id = ? AND image_path <> ''")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;

    let pages_with_ocr: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pages WHERE project_id = ? AND ocr_text IS NOT NULL AND ocr_text <> ''",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;

    let round_incomplete: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM page_rounds pr
         JOIN pages pg ON pg.id = pr.page_id
         WHERE pg.project_id = ?
           AND pr.round = ?
           AND pr.state <> 'done'",
    )
    .bind(project_id)
    .bind(current_round.as_ref().map_or("", RoundId::as_str))
    .fetch_one(&mut *tx)
    .await?;

    let ctx = ProjectContext {
        id: ProjectId::new(pid),
        phase,
        current_round,
        on_hold: on_hold != 0,
        total_pages: total_pages.0 as usize,
        pages_with_image: pages_with_image.0 as usize,
        pages_with_ocr: pages_with_ocr.0 as usize,
        round_incomplete_pages: round_incomplete.0 as usize,
    };

    // Pure domain evaluation
    let effects = apply(&ctx, target_phase, actor_role, actor_id, rounds_config)?;

    // Execute effects
    execute_effects_tx(&mut tx, &effects).await?;

    tx.commit().await?;

    Ok(effects)
}

/// Atomically toggle a project's `on_hold` flag.
///
/// # Errors
/// Returns [`DbError::Conflict`] if project is not found or user lacks permission,
/// or [`DbError::Sqlx`] on DB failure.
pub async fn toggle_project_hold(
    pool: &MySqlPool,
    project_id: u64,
    actor_role: ActorRole,
    actor_id: Option<UserId>,
) -> Result<bool, DbError> {
    let mut tx = pool.begin().await?;

    let proj: Option<(u64, String, Option<String>, i8)> = sqlx::query_as(
        "SELECT id, phase, current_round, on_hold
         FROM projects
         WHERE id = ?
         FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((pid, phase_str, current_round_str, on_hold)) = proj else {
        tx.rollback().await?;
        return Err(DbError::Conflict("dự án không tồn tại".to_string()));
    };

    let phase = Phase::from_str(&phase_str)
        .map_err(|e| DbError::Conflict(format!("trạng thái phase không hợp lệ: {e}")))?;

    let current_round = match current_round_str {
        Some(s) if !s.is_empty() => Some(
            RoundId::new(s)
                .map_err(|e| DbError::Conflict(format!("round id không hợp lệ: {e}")))?,
        ),
        _ => None,
    };

    let ctx = ProjectContext {
        id: ProjectId::new(pid),
        phase,
        current_round,
        on_hold: on_hold != 0,
        total_pages: 0,
        pages_with_image: 0,
        pages_with_ocr: 0,
        round_incomplete_pages: 0,
    };

    let effects = toggle_on_hold(&ctx, actor_role, actor_id)?;
    execute_effects_tx(&mut tx, &effects).await?;

    tx.commit().await?;

    Ok(!ctx.on_hold)
}
