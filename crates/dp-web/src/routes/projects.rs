//! Routes and handlers for project creation, import, and detail view.

use std::str::FromStr;

use axum::{
    Form,
    extract::{Multipart, Path, State},
    response::{IntoResponse, Redirect, Response},
};
use dp_core::{ActorRole, Phase, UserId, round::RoundsConfig};
use dp_db::queries::{
    page_rounds::{claim_next_available_page, get_user_active_claim},
    page_tallies::check_daily_limit,
    pages::count_pages_by_project_id,
    projects::{
        NewProject, create_project, get_project_by_id, get_project_detail, list_projects,
        toggle_project_hold, transition_project,
    },
};
use serde::Deserialize;
use tracing::instrument;

use crate::{
    AppError, AppState,
    auth::{AuthUser, ManagerUser, verify_csrf_token},
    ingest::{self, preview::IngestPreview},
};

// ─── New project form ────────────────────────────────────────────────────────

/// `GET /projects/new` — show project creation form.
#[instrument(skip(state))]
pub async fn new_project_page(
    manager: ManagerUser,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let tmpl = state.templates.get_template("projects/form.html")?;
    let ctx = minijinja::context! {
        user => minijinja::context! {
            username => manager.0.username.clone(),
            roles => manager.0.roles.clone(),
        },
        csrf_token => manager.0.csrf_token.clone(),
        error => minijinja::Value::UNDEFINED,
    };
    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
}

/// Form data for creating a new project.
#[derive(Debug, Deserialize)]
pub struct NewProjectForm {
    pub name: String,
    pub author: String,
    pub tone_style: String,
    pub charset: String,
    pub image_source: String,
    pub comments: String,
    pub csrf_token: String,
}

/// `POST /projects/new` — create the project and redirect to its import page.
#[instrument(skip(state, form))]
pub async fn new_project_submit(
    manager: ManagerUser,
    State(state): State<AppState>,
    Form(form): Form<NewProjectForm>,
) -> Result<Response, AppError> {
    // CSRF
    if !crate::auth::csrf::verify_csrf_token(&form.csrf_token, &manager.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let name = form.name.trim().to_owned();
    let author = form.author.trim().to_owned();
    if name.is_empty() {
        let tmpl = state.templates.get_template("projects/form.html")?;
        let ctx = minijinja::context! {
            user => minijinja::context! {
                username => manager.0.username.clone(),
                roles => manager.0.roles.clone(),
            },
            csrf_token => manager.0.csrf_token.clone(),
            error => "Tên dự án không được để trống.",
        };
        return Ok(axum::response::Html(tmpl.render(ctx)?).into_response());
    }

    let tone_style = validate_tone_style(&form.tone_style)?;
    let charset = validate_charset(&form.charset)?;

    let new_proj = NewProject {
        name: &name,
        author: &author,
        tone_style,
        charset,
        manager_id: manager.0.user_id.0,
        image_source: form.image_source.trim(),
        comments: form.comments.trim(),
    };

    let project_id = create_project(&state.db, &new_proj).await?;

    Ok(Redirect::to(&format!("/projects/{project_id}/import")).into_response())
}

// ─── Project detail ───────────────────────────────────────────────────────────

/// `GET /projects/{id}` — project detail page.
#[instrument(skip(state), fields(project_id = id))]
pub async fn project_detail(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    let project = get_project_detail(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let page_count = count_pages_by_project_id(&state.db, id).await?;

    let tmpl = state.templates.get_template("projects/detail.html")?;
    let ctx = minijinja::context! {
        user => minijinja::context! {
            username => auth.username.clone(),
            roles => auth.roles.clone(),
        },
        csrf_token => auth.csrf_token.clone(),
        project => minijinja::context! {
            id => project.id,
            name => project.name.clone(),
            author => project.author.clone(),
            phase => project.phase.clone(),
            current_round => project.current_round.clone(),
            on_hold => project.on_hold,
            tone_style => project.tone_style.clone(),
            charset => project.charset.clone(),
            manager_id => project.manager_id,
            manager_username => project.manager_username.clone(),
            created_at => project.created_at.to_rfc3339(),
            updated_at => project.updated_at.to_rfc3339(),
        },
        page_count,
    };
    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
}

// ─── Import upload ────────────────────────────────────────────────────────────

/// `GET /projects/{id}/import` — show import form.
#[instrument(skip(state), fields(project_id = id))]
pub async fn import_page(
    manager: ManagerUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    let project = get_project_detail(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Only the project manager or admin can import
    if project.manager_id != manager.0.user_id.0 && !manager.0.roles.iter().any(|r| r == "admin") {
        return Err(AppError::Forbidden);
    }

    if project.phase != "draft" {
        return Err(AppError::BadRequest(format!(
            "Dự án đang ở trạng thái '{}', chỉ dự án nháp mới có thể nhập dữ liệu.",
            project.phase
        )));
    }

    let tmpl = state.templates.get_template("projects/import.html")?;
    let ctx = minijinja::context! {
        user => minijinja::context! {
            username => manager.0.username.clone(),
            roles => manager.0.roles.clone(),
        },
        csrf_token => manager.0.csrf_token.clone(),
        project => minijinja::context! {
            id => project.id,
            name => project.name.clone(),
            phase => project.phase.clone(),
        },
        preview => minijinja::Value::UNDEFINED,
        error => minijinja::Value::UNDEFINED,
    };
    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
}

/// `POST /projects/{id}/import` — receive zip, run dry-run preview or commit.
#[instrument(skip(state, multipart), fields(project_id = id))]
pub async fn import_submit(
    manager: ManagerUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let project = get_project_detail(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if project.manager_id != manager.0.user_id.0 && !manager.0.roles.iter().any(|r| r == "admin") {
        return Err(AppError::Forbidden);
    }

    if project.phase != "draft" {
        return Err(AppError::BadRequest(format!(
            "Dự án đang ở trạng thái '{}', chỉ dự án nháp mới có thể nhập dữ liệu.",
            project.phase
        )));
    }

    // Extract multipart fields
    let mut zip_bytes: Option<axum::body::Bytes> = None;
    let mut action = "preview".to_owned();
    let mut replace = false;
    let mut csrf_value = String::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("lỗi đọc form upload: {e}")))?
    {
        match field.name().unwrap_or("") {
            "csrf_token" => {
                csrf_value = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
            }
            "action" => {
                action = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
            }
            "replace" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                replace = v == "1" || v.to_lowercase() == "true";
            }
            "zip_file" => {
                let b = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("lỗi đọc tập tin zip: {e}")))?;
                zip_bytes = Some(b);
            }
            _ => {}
        }
    }

    if !crate::auth::csrf::verify_csrf_token(&csrf_value, &manager.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let zip_bytes = zip_bytes
        .ok_or_else(|| AppError::BadRequest("Vui lòng chọn tập tin zip để tải lên.".to_owned()))?;

    let project_id_val = project.id;
    let project_name = project.name.clone();
    let project_phase = project.phase.clone();

    match action.as_str() {
        "commit" => {
            let data_dir = state.config.paths.data.to_string_lossy().to_string();
            match ingest::commit(&state.db, &zip_bytes, project_id_val, &data_dir, replace).await {
                Ok(inserted) => {
                    tracing::info!(
                        project_id = project_id_val,
                        pages = inserted.len(),
                        "ingest committed"
                    );
                    Ok(Redirect::to(&format!("/projects/{project_id_val}")).into_response())
                }
                Err(e) => {
                    let tmpl = state.templates.get_template("projects/import.html")?;
                    let ctx = minijinja::context! {
                        user => minijinja::context! {
                            username => manager.0.username.clone(),
                            roles => manager.0.roles.clone(),
                        },
                        csrf_token => manager.0.csrf_token.clone(),
                        project => minijinja::context! {
                            id => project_id_val,
                            name => project_name.clone(),
                            phase => project_phase.clone(),
                        },
                        preview => minijinja::Value::UNDEFINED,
                        error => e.to_string(),
                    };
                    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
                }
            }
        }
        _ => {
            // Default: dry-run preview
            match ingest::dry_run(&zip_bytes).await {
                Ok(preview) => {
                    let tmpl = state.templates.get_template("projects/import.html")?;
                    let ctx = build_import_ctx_with_preview(
                        &manager,
                        project_id_val,
                        &project_name,
                        &project_phase,
                        preview,
                    );
                    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
                }
                Err(e) => {
                    let tmpl = state.templates.get_template("projects/import.html")?;
                    let ctx = minijinja::context! {
                        user => minijinja::context! {
                            username => manager.0.username.clone(),
                            roles => manager.0.roles.clone(),
                        },
                        csrf_token => manager.0.csrf_token.clone(),
                        project => minijinja::context! {
                            id => project_id_val,
                            name => project_name,
                            phase => project_phase,
                        },
                        preview => minijinja::Value::UNDEFINED,
                        error => e.to_string(),
                    };
                    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
                }
            }
        }
    }
}

// ─── Project list ─────────────────────────────────────────────────────────────

/// `GET /projects` — list of available projects.
#[instrument(skip(state))]
pub async fn project_list(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let projects = list_projects(&state.db, None, None, 50, 0).await?;

    let tmpl = state.templates.get_template("projects/list.html")?;
    let ctx = minijinja::context! {
        user => minijinja::context! {
            username => auth.username.clone(),
            roles => auth.roles.clone(),
        },
        csrf_token => auth.csrf_token.clone(),
        projects => projects.iter().map(|p| minijinja::context! {
            id => p.id,
            name => p.name.clone(),
            author => p.author.clone(),
            phase => p.phase.clone(),
            current_round => p.current_round.clone(),
            manager_username => p.manager_username.clone(),
            total_pages => p.total_pages,
            created_at => p.created_at.to_rfc3339(),
        }).collect::<Vec<_>>(),
    };
    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
}

/// `GET /my/projects` — projects managed by the current user.
#[instrument(skip(state))]
pub async fn my_projects(
    manager: ManagerUser,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let projects = list_projects(&state.db, None, Some(manager.0.user_id.0), 50, 0).await?;

    let tmpl = state.templates.get_template("projects/list.html")?;
    let ctx = minijinja::context! {
        user => minijinja::context! {
            username => manager.0.username.clone(),
            roles => manager.0.roles.clone(),
        },
        csrf_token => manager.0.csrf_token.clone(),
        projects => projects.iter().map(|p| minijinja::context! {
            id => p.id,
            name => p.name.clone(),
            author => p.author.clone(),
            phase => p.phase.clone(),
            current_round => p.current_round.clone(),
            manager_username => p.manager_username.clone(),
            total_pages => p.total_pages,
            created_at => p.created_at.to_rfc3339(),
        }).collect::<Vec<_>>(),
        my_projects => true,
    };
    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn build_import_ctx_with_preview(
    manager: &ManagerUser,
    project_id: u64,
    project_name: &str,
    project_phase: &str,
    preview: IngestPreview,
) -> minijinja::Value {
    minijinja::context! {
        user => minijinja::context! {
            username => manager.0.username.clone(),
            roles => manager.0.roles.clone(),
        },
        csrf_token => manager.0.csrf_token.clone(),
        project => minijinja::context! {
            id => project_id,
            name => project_name.to_owned(),
            phase => project_phase.to_owned(),
        },
        preview => minijinja::context! {
            total_pages => preview.total_pages,
            warning_pages => preview.warning_pages,
            error_count => preview.error_count,
            can_commit => preview.can_commit(),
            has_warnings => preview.has_warnings(),
            errors => preview.errors,
            global_warnings => preview.global_warnings,
            pages => preview.pages.iter().take(50).map(|p| minijinja::context! {
                page_number => p.page_number,
                image_entry => p.image_entry.clone(),
                width => p.width,
                height => p.height,
                first_lines => p.first_lines.clone(),
                warnings => p.warnings.clone(),
            }).collect::<Vec<_>>(),
        },
        error => minijinja::Value::UNDEFINED,
    }
}

fn validate_tone_style(s: &str) -> Result<&'static str, AppError> {
    match s.trim() {
        "old" => Ok("old"),
        "new" => Ok("new"),
        "either" | "" => Ok("either"),
        other => Err(AppError::BadRequest(format!(
            "Giá trị tone_style không hợp lệ: '{other}'"
        ))),
    }
}

fn validate_charset(s: &str) -> Result<&'static str, AppError> {
    match s.trim() {
        "vi" | "" => Ok("vi"),
        "vi-fr" => Ok("vi-fr"),
        other => Err(AppError::BadRequest(format!(
            "Bộ ký tự không được hỗ trợ: '{other}'"
        ))),
    }
}

// ─── Phase transitions & hold ────────────────────────────────────────────────

/// Form data for transitioning a project phase.
#[derive(Debug, Deserialize)]
pub struct TransitionForm {
    pub target_phase: String,
    pub csrf_token: String,
}

/// `POST /projects/{id}/transition` — execute a project state transition.
#[instrument(skip(state, form))]
pub async fn project_transition_submit(
    manager: ManagerUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<TransitionForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &manager.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let target_phase = Phase::from_str(&form.target_phase)
        .map_err(|e| AppError::BadRequest(format!("Trạng thái không hợp lệ: {e}")))?;

    let actor_role = if manager.0.has_role("admin") {
        ActorRole::Admin
    } else {
        ActorRole::Manager
    };

    let rounds = RoundsConfig::default_embedded();
    transition_project(
        &state.db,
        id,
        target_phase,
        actor_role,
        Some(UserId::new(manager.0.user_id.0)),
        &rounds,
    )
    .await?;

    Ok(Redirect::to(&format!("/projects/{id}")).into_response())
}

/// Form data for toggling project hold status.
#[derive(Debug, Deserialize)]
pub struct ToggleHoldForm {
    pub csrf_token: String,
}

/// `POST /projects/{id}/hold` — toggle project on_hold pause flag.
#[instrument(skip(state, form))]
pub async fn project_toggle_hold_submit(
    manager: ManagerUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<ToggleHoldForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &manager.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let actor_role = if manager.0.has_role("admin") {
        ActorRole::Admin
    } else {
        ActorRole::Manager
    };

    toggle_project_hold(
        &state.db,
        id,
        actor_role,
        Some(UserId::new(manager.0.user_id.0)),
    )
    .await?;

    Ok(Redirect::to(&format!("/projects/{id}")).into_response())
}

/// Form data for claiming a page.
#[derive(Debug, Deserialize)]
pub struct ClaimPageForm {
    pub csrf_token: String,
}

/// `POST /projects/{id}/claim` — claim the next available page in active round.
#[instrument(skip(state, form))]
pub async fn project_claim_submit(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<ClaimPageForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    // Check daily limit
    check_daily_limit(&state.db, auth.user_id.0, state.config.limits.daily_pages).await?;

    // Check if user already holds a page in progress
    if let Some(active) = get_user_active_claim(&state.db, auth.user_id.0).await? {
        return Ok(Redirect::to(&format!(
            "/projects/{}/proof/{}",
            active.project_id, active.page_number
        ))
        .into_response());
    }

    let project = get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if project.phase != "in_round" || project.on_hold != 0 {
        return Err(AppError::BadRequest(
            "Dự án hiện không nhận soát lỗi.".to_string(),
        ));
    }

    let current_round = project
        .current_round
        .ok_or_else(|| AppError::BadRequest("Dự án không có vòng hoạt động.".to_string()))?;

    let rounds = RoundsConfig::default_embedded();
    let prev_round_opt = rounds
        .get_round(
            &dp_core::RoundId::new(&current_round)
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
        )
        .and_then(|r| rounds.prev_round(&r.id))
        .map(|r| r.id.as_str());

    let claimed = claim_next_available_page(
        &state.db,
        id,
        &current_round,
        prev_round_opt,
        auth.user_id.0,
        3,
    )
    .await?;

    match claimed {
        Some(page) => {
            Ok(Redirect::to(&format!("/projects/{id}/proof/{}", page.page_number)).into_response())
        }
        None => Ok(Redirect::to(&format!("/projects/{id}")).into_response()),
    }
}
