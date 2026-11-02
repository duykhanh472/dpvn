//! Word lists management routes for project managers and site administrators (§9.2, §7.2).

use axum::{
    extract::{Form, Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use dp_core::viet;
use dp_db::queries::{
    admin_audit::{NewAdminAction, record_admin_action},
    projects::get_project_by_id,
    words::{
        add_project_word, add_site_word, count_project_words, count_site_words, list_project_words,
        list_site_words, remove_project_word, remove_site_word,
    },
};
use minijinja::context;
use serde::Deserialize;
use tracing::instrument;

use crate::{
    app::{AppError, AppState},
    auth::{AuthUser, csrf::verify_csrf_token},
};

#[derive(Debug, Deserialize)]
pub struct WordListQuery {
    pub kind: Option<String>,
    pub q: Option<String>,
    pub page: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct WordActionForm {
    pub action: String,
    pub kind: String,
    pub word: String,
    /// Explicit confirmation for destructive deletes.
    #[serde(default)]
    pub confirm: Option<String>,
    pub csrf_token: String,
}

/// Require the explicit confirmation field used by destructive forms.
fn require_confirm(value: Option<&str>) -> Result<(), AppError> {
    match value.map(str::trim) {
        Some("1" | "true" | "on" | "yes") => Ok(()),
        _ => Err(AppError::BadRequest(
            "Vui lòng xác nhận thao tác phá hủy này.".to_string(),
        )),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Project Custom Word Lists (§9.2)
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /projects/{id}/words` — Manage custom good/bad words for a specific project.
#[instrument(skip(state, auth))]
pub async fn project_words_page(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<WordListQuery>,
) -> Result<Response, AppError> {
    let project = get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let is_manager = project.manager_id == auth.user_id.0 || auth.has_role("admin");
    if !is_manager {
        return Err(AppError::Forbidden);
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = 50;
    let offset = (page - 1) * per_page;

    let kind_filter = query
        .kind
        .as_deref()
        .filter(|k| *k == "good" || *k == "bad");
    let search_filter = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let words =
        list_project_words(&state.db, id, kind_filter, search_filter, per_page, offset).await?;
    let total_count = count_project_words(&state.db, id, kind_filter, search_filter).await?;
    let total_pages = (total_count as f64 / per_page as f64).ceil() as u32;

    let tmpl = state.templates.get_template("projects/words.html")?;
    let words_ctx: Vec<_> = words
        .into_iter()
        .map(|w| {
            context! {
                kind => w.kind,
                word => w.word,
                added_by => w.added_by_username.unwrap_or_else(|| "Tự động".to_string()),
                added_at => w.added_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect();

    let rendered = tmpl.render(context! {
        project_id => id,
        project_name => project.name,
        words => words_ctx,
        total_count => total_count,
        current_page => page,
        total_pages => total_pages.max(1),
        filter_kind => query.kind.unwrap_or_default(),
        filter_q => query.q.unwrap_or_default(),
        csrf_token => auth.csrf_token,
        user => context! {
            username => auth.username,
            roles => auth.roles,
        },
    })?;

    Ok(Html(rendered).into_response())
}

/// `POST /projects/{id}/words` — Add or remove a project word.
#[instrument(skip(state, auth, form))]
pub async fn project_words_submit(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<WordActionForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let project = get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let is_manager = project.manager_id == auth.user_id.0 || auth.has_role("admin");
    if !is_manager {
        return Err(AppError::Forbidden);
    }

    let normalized = viet::normalize(&form.word).trim().to_lowercase();
    if normalized.is_empty() {
        return Err(AppError::BadRequest("Từ không được để trống.".to_string()));
    }

    if form.kind != "good" && form.kind != "bad" {
        return Err(AppError::BadRequest(
            "Loại từ phải là 'good' hoặc 'bad'.".to_string(),
        ));
    }

    match form.action.as_str() {
        "add" => {
            add_project_word(&state.db, id, &form.kind, &normalized, Some(auth.user_id.0)).await?;
        }
        "delete" => {
            require_confirm(form.confirm.as_deref())?;
            remove_project_word(&state.db, id, &form.kind, &normalized).await?;
        }
        _ => return Err(AppError::BadRequest("Thao tác không hợp lệ.".to_string())),
    }

    Ok(Redirect::to(&format!("/projects/{id}/words")).into_response())
}

// ─────────────────────────────────────────────────────────────────────────────
// Site Words Administration (§9.2, §7.2)
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /admin/words` — Manage site-wide dictionary (Administrators only).
#[instrument(skip(state, auth))]
pub async fn admin_words_page(
    auth: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<WordListQuery>,
) -> Result<Response, AppError> {
    if !auth.has_role("admin") {
        return Err(AppError::Forbidden);
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = 50;
    let offset = (page - 1) * per_page;

    let kind_filter = query
        .kind
        .as_deref()
        .filter(|k| *k == "good" || *k == "bad");
    let search_filter = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let words = list_site_words(&state.db, kind_filter, search_filter, per_page, offset).await?;
    let total_count = count_site_words(&state.db, kind_filter, search_filter).await?;
    let total_pages = (total_count as f64 / per_page as f64).ceil() as u32;

    let tmpl = state.templates.get_template("admin/words.html")?;
    let words_ctx: Vec<_> = words
        .into_iter()
        .map(|w| {
            context! {
                kind => w.kind,
                word => w.word,
                added_by => w.added_by_username.unwrap_or_else(|| "Hệ thống".to_string()),
                added_at => w.added_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect();

    let rendered = tmpl.render(context! {
        words => words_ctx,
        total_count => total_count,
        current_page => page,
        total_pages => total_pages.max(1),
        filter_kind => query.kind.unwrap_or_default(),
        filter_q => query.q.unwrap_or_default(),
        csrf_token => auth.csrf_token,
        user => context! {
            username => auth.username,
            roles => auth.roles,
        },
    })?;

    Ok(Html(rendered).into_response())
}

/// `POST /admin/words` — Add or remove a site-wide dictionary entry.
#[instrument(skip(state, auth, form))]
pub async fn admin_words_submit(
    auth: AuthUser,
    State(state): State<AppState>,
    Form(form): Form<WordActionForm>,
) -> Result<Response, AppError> {
    if !auth.has_role("admin") {
        return Err(AppError::Forbidden);
    }

    if !verify_csrf_token(&form.csrf_token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let normalized = viet::normalize(&form.word).trim().to_lowercase();
    if normalized.is_empty() {
        return Err(AppError::BadRequest("Từ không được để trống.".to_string()));
    }

    if form.kind != "good" && form.kind != "bad" {
        return Err(AppError::BadRequest(
            "Loại từ phải là 'good' hoặc 'bad'.".to_string(),
        ));
    }

    match form.action.as_str() {
        "add" => {
            add_site_word(&state.db, &form.kind, &normalized, Some(auth.user_id.0)).await?;
        }
        "delete" => {
            require_confirm(form.confirm.as_deref())?;
            remove_site_word(&state.db, &form.kind, &normalized).await?;
        }
        _ => return Err(AppError::BadRequest("Thao tác không hợp lệ.".to_string())),
    }

    // Record the administrative action before refreshing the cache.
    let (action, verb) = if form.action == "add" {
        ("site_word.add", "thêm")
    } else {
        ("site_word.delete", "xóa")
    };
    let detail = format!(
        "{verb} từ '{}' ({}) vào từ điển hệ thống",
        normalized, form.kind
    );
    record_admin_action(
        &state.db,
        &NewAdminAction {
            actor_id: auth.user_id.0,
            actor_username: &auth.username,
            action,
            target_type: "site_word",
            target_id: None,
            detail: &detail,
        },
    )
    .await?;

    // Refresh in-memory site_words cache (§9.6)
    if let Ok((good, bad)) = dp_db::queries::words::get_all_site_words(&state.db).await {
        let mut guard = state.site_words.write().await;
        *guard = (good, bad);
    }

    Ok(Redirect::to("/admin/words").into_response())
}
