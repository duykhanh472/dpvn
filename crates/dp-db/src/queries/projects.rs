//! Database queries for project records, metadata, and phase updates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, MySqlPool};

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
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id), 0) AS total_pages,
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

/// List projects with optional phase and manager filters.
///
/// # Errors
/// Returns [`DbError::Sqlx`] if database query fails.
pub async fn list_projects(
    pool: &MySqlPool,
    phase: Option<&str>,
    manager_id: Option<u64>,
    limit: u32,
    offset: u32,
) -> Result<Vec<ProjectListItem>, DbError> {
    let rows = sqlx::query_as::<_, ProjectListItem>(
        "SELECT p.id, p.name, p.author, p.phase, p.current_round, p.on_hold,
                p.tone_style, p.charset, p.manager_id, u.username AS manager_username,
                COALESCE((SELECT COUNT(*) FROM pages pg WHERE pg.project_id = p.id), 0) AS total_pages,
                p.created_at, p.updated_at
         FROM projects p
         JOIN users u ON u.id = p.manager_id
         WHERE (? IS NULL OR p.phase = ?)
           AND (? IS NULL OR p.manager_id = ?)
         ORDER BY p.updated_at DESC
         LIMIT ? OFFSET ?",
    )
    .bind(phase)
    .bind(phase)
    .bind(manager_id)
    .bind(manager_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows)
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
