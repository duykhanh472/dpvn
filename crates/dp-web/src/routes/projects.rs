//! Routes and handlers for project creation, import, and detail view.

use std::str::FromStr;

use axum::{
    Form,
    extract::{Multipart, Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
};
use dp_core::{ActorRole, Phase, RoundsConfig, UserId};
use dp_db::DbError;
use dp_db::queries::{
    comments::{count_project_comments, list_project_comments},
    page_rounds::{claim_next_available_page, get_user_active_claim},
    page_tallies::{check_daily_limit, current_vietnam_date, get_total_daily_tally},
    project_events::list_project_events_with_actors,
    projects::{
        NewProject, ProjectListFilter, count_projects_filtered, create_project, get_project_by_id,
        get_project_detail, get_project_progress, list_projects_filtered, toggle_project_hold,
        transition_project,
    },
};
use serde::Deserialize;
use tracing::instrument;

use crate::{
    AppError, AppState,
    auth::{AuthUser, ManagerUser, verify_csrf_token},
    format_vietnam_time,
    ingest::{self, preview::IngestPreview},
    settings::SiteSettings,
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

/// Query parameters accepted by the project detail page.
#[derive(Debug, Default, Deserialize)]
pub struct ProjectDetailQuery {
    /// Optional notice code (e.g. `limit_reached`, `no_pages`).
    pub notice: Option<String>,
    /// Legacy empty-pool flag from the proof entry redirect.
    pub empty: Option<String>,
}

/// `GET /projects/{id}` — project detail page with progress, events and comments.
#[instrument(skip(state, auth), fields(project_id = id))]
pub async fn project_detail(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<ProjectDetailQuery>,
) -> Result<Response, AppError> {
    let project = get_project_detail(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let progress = get_project_progress(&state.db, id, project.current_round.as_deref()).await?;
    let site_settings = SiteSettings::load(&state.db, &state.config.limits).await?;
    let daily_tally =
        get_total_daily_tally(&state.db, auth.user_id.0, current_vietnam_date()).await?;
    let daily_limit = site_settings.daily_pages;
    let limit_reached = daily_tally >= daily_limit;

    let notice = match (query.notice.as_deref(), query.empty.as_deref()) {
        (Some("limit_reached"), _) => Some(format!(
            "Bạn đã đạt giới hạn {daily_limit} trang trong ngày. Giới hạn sẽ được đặt lại vào 00:00 giờ Việt Nam."
        )),
        (Some("no_pages"), _) | (_, Some("1")) => Some(
            "Hiện không còn trang nào phù hợp để bạn soát trong vòng này. Hãy quay lại sau hoặc chọn dự án khác."
                .to_string(),
        ),
        _ => None,
    };

    let can_manage = project.manager_id == auth.user_id.0 || auth.has_role("admin");

    let comments = list_project_comments(&state.db, id, 50, 0).await?;
    let comment_count = count_project_comments(&state.db, id).await?;
    let comment_ctx: Vec<_> = comments
        .iter()
        .map(|comment| {
            minijinja::context! {
                id => comment.id,
                body_html => comment.body_html.clone(),
                author_username => comment.author_username.clone(),
                created_at_label => format_vietnam_time(comment.created_at, "%d/%m/%Y %H:%M"),
            }
        })
        .collect();

    // Plan Q6: the audit log is visible to managers and admins only.
    let events = if can_manage {
        list_project_events_with_actors(&state.db, id, 20).await?
    } else {
        Vec::new()
    };
    let event_ctx: Vec<_> = events
        .iter()
        .map(|event| {
            minijinja::context! {
                from_phase => event.from_phase.clone(),
                to_phase => event.to_phase.clone(),
                from_round => event.from_round.clone(),
                to_round => event.to_round.clone(),
                actor_username => event.actor_username.clone().unwrap_or_else(|| "Hệ thống".to_string()),
                note => event.note.clone(),
                created_at_label => format_vietnam_time(event.created_at, "%d/%m/%Y %H:%M"),
            }
        })
        .collect();

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
            image_source => project.image_source.clone(),
            comments => project.comments.clone(),
            created_at => project.created_at.to_rfc3339(),
            updated_at => project.updated_at.to_rfc3339(),
            created_at_label => format_vietnam_time(project.created_at, "%d/%m/%Y"),
            updated_at_label => format_vietnam_time(project.updated_at, "%d/%m/%Y %H:%M"),
        },
        progress => minijinja::context! {
            total_pages => progress.total_pages,
            available_pages => progress.available_pages,
            in_progress_pages => progress.in_progress_pages,
            done_pages => progress.done_pages,
            percent => progress.percent,
        },
        page_count => progress.total_pages,
        comments => comment_ctx,
        comment_count => comment_count,
        events => event_ctx,
        can_view_events => can_manage,
        can_manage => can_manage,
        daily_tally => daily_tally,
        daily_limit => daily_limit,
        limit_reached => limit_reached,
        notice => notice.unwrap_or_default(),
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

/// Supported page sizes for the project list.
const PER_PAGE_OPTIONS: [u32; 4] = [10, 25, 50, 100];
/// Default page size.
const DEFAULT_PER_PAGE: u32 = 25;
/// Hard cap on page size (§13.2, Phase 8 acceptance).
const MAX_PER_PAGE: u32 = 100;

/// Query parameters for the project list (filters are shareable via URL).
#[derive(Debug, Default, Deserialize)]
pub struct ProjectListQuery {
    /// Round filter (`P1`, `P2`, …) or `all`.
    pub round: Option<String>,
    /// Availability filter (`available`, `in_progress`, `complete`, `finished`, `all`).
    pub availability: Option<String>,
    /// Sort key (`recent`, `created`, `name`, `pages`, `pages_asc`).
    pub sort: Option<String>,
    /// Page number, 1-based.
    pub page: Option<u32>,
    /// Page size, capped at [`MAX_PER_PAGE`].
    pub per: Option<u32>,
    /// Substring search over project name and author.
    pub q: Option<String>,
}

/// Which project list is being rendered.
#[derive(Debug, Clone, Copy)]
struct ListMode {
    base_path: &'static str,
    my_projects: bool,
    default_availability: &'static str,
}

/// Percent-encode a query string component (RFC 3986 unreserved set).
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Build a project-list URL preserving every active filter.
fn list_url(
    base_path: &str,
    availability: &str,
    round: Option<&str>,
    sort: &str,
    search: Option<&str>,
    per: u32,
    page: u32,
) -> String {
    let mut url = format!(
        "{base_path}?availability={}&sort={}&per={per}&page={page}",
        encode_component(availability),
        encode_component(sort),
    );
    if let Some(round) = round {
        url.push_str("&round=");
        url.push_str(&encode_component(round));
    }
    if let Some(search) = search {
        url.push_str("&q=");
        url.push_str(&encode_component(search));
    }
    url
}

/// Render the project list (full page or HTMX results fragment).
async fn render_project_list(
    auth: AuthUser,
    state: &AppState,
    query: ProjectListQuery,
    mode: ListMode,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let rounds_config = RoundsConfig::default_embedded();

    // ── Normalize filters ────────────────────────────────────────────────────
    let availability = query
        .availability
        .as_deref()
        .unwrap_or(mode.default_availability);
    let availability = match availability {
        "available" | "in_progress" | "complete" | "finished" | "all" => availability,
        _ => mode.default_availability,
    };
    let sort = query.sort.as_deref().unwrap_or("recent");
    let sort = match sort {
        "created" | "name" | "pages" | "pages_asc" | "recent" => sort,
        _ => "recent",
    };

    let round_raw = query
        .round
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != "all");
    let round = round_raw.filter(|r| {
        rounds_config
            .rounds
            .iter()
            .any(|candidate| candidate.enabled && candidate.id.as_str() == *r)
    });
    let round = round.map(str::to_owned);

    let search_normalized = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(dp_core::viet::normalize);
    let search = search_normalized.as_deref();

    let per = query.per.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
    let requested_page = query.page.unwrap_or(1).max(1);

    // ── Count first so the page number can be clamped (no off-by-one). ───────
    let count_filter = ProjectListFilter {
        manager_id: mode.my_projects.then_some(auth.user_id.0),
        round: round.as_deref(),
        phase: None,
        availability: (availability != "all").then_some(availability),
        search,
        sort,
        limit: 0,
        offset: 0,
    };
    let total_count = count_projects_filtered(&state.db, &count_filter).await?;

    let total_pages_count = if total_count == 0 {
        1
    } else {
        ((total_count + i64::from(per) - 1) / i64::from(per)) as u32
    };
    let page = requested_page.min(total_pages_count);
    let offset = (page - 1) * per;

    let filter = ProjectListFilter {
        limit: per,
        offset,
        ..count_filter
    };
    let projects = list_projects_filtered(&state.db, &filter).await?;

    // ── Shared context fragments ─────────────────────────────────────────────
    let project_ctx: Vec<_> = projects
        .iter()
        .map(|project| {
            minijinja::context! {
                id => project.id,
                name => project.name.clone(),
                author => project.author.clone(),
                phase => project.phase.clone(),
                current_round => project.current_round.clone(),
                on_hold => project.on_hold,
                tone_style => project.tone_style.clone(),
                manager_username => project.manager_username.clone(),
                total_pages => project.total_pages,
                available_pages => project.available_pages,
                in_progress_pages => project.in_progress_pages,
                done_pages => project.done_pages,
                created_at => project.created_at.to_rfc3339(),
                created_at_label => format_vietnam_time(project.created_at, "%d/%m/%Y"),
                updated_at_label => format_vietnam_time(project.updated_at, "%d/%m/%Y %H:%M"),
            }
        })
        .collect();

    let round_options: Vec<_> = std::iter::once(minijinja::context! {
        value => "all",
        label => "Tất cả các vòng",
        selected => round.is_none(),
    })
    .chain(rounds_config.rounds.iter().filter(|r| r.enabled).map(|r| {
        minijinja::context! {
            value => r.id.as_str(),
            label => format!("Vòng {}", r.id),
            selected => round.as_deref() == Some(r.id.as_str()),
        }
    }))
    .collect();

    let availability_options: Vec<_> = [
        ("available", "Có trang chờ soát"),
        ("in_progress", "Đang có người soát"),
        ("complete", "Đã soát xong vòng hiện tại"),
        ("finished", "Đã hoàn thành"),
        ("all", "Tất cả trạng thái"),
    ]
    .into_iter()
    .map(|(value, label)| {
        minijinja::context! {
            value => value,
            label => label,
            selected => availability == value,
        }
    })
    .collect();

    let sort_options: Vec<_> = [
        ("recent", "Cập nhật gần đây"),
        ("created", "Mới tạo gần đây"),
        ("name", "Tên A–Z"),
        ("pages", "Nhiều trang trước"),
        ("pages_asc", "Ít trang trước"),
    ]
    .into_iter()
    .map(|(value, label)| {
        minijinja::context! {
            value => value,
            label => label,
            selected => sort == value,
        }
    })
    .collect();

    let per_options: Vec<_> = PER_PAGE_OPTIONS
        .into_iter()
        .map(|value| {
            minijinja::context! {
                value => value,
                label => format!("{value} / trang"),
                selected => per == value,
            }
        })
        .collect();

    let previous_url = (page > 1).then(|| {
        list_url(
            mode.base_path,
            availability,
            round.as_deref(),
            sort,
            search,
            per,
            page - 1,
        )
    });
    let next_url = (page < total_pages_count).then(|| {
        list_url(
            mode.base_path,
            availability,
            round.as_deref(),
            sort,
            search,
            per,
            page + 1,
        )
    });

    let mut page_links = Vec::new();
    let window_start = page.saturating_sub(2).max(1);
    let window_end = (window_start + 4).min(total_pages_count);
    let window_start = window_end.saturating_sub(4).max(1);
    for number in window_start..=window_end {
        page_links.push(minijinja::context! {
            number => number,
            url => list_url(mode.base_path, availability, round.as_deref(), sort, search, per, number),
            current => number == page,
        });
    }

    let daily_tally =
        get_total_daily_tally(&state.db, auth.user_id.0, current_vietnam_date()).await?;
    let daily_limit = SiteSettings::load(&state.db, &state.config.limits)
        .await?
        .daily_pages;

    let ctx = minijinja::context! {
        user => minijinja::context! {
            username => auth.username.clone(),
            roles => auth.roles.clone(),
        },
        csrf_token => auth.csrf_token.clone(),
        projects => project_ctx,
        my_projects => mode.my_projects,
        base_path => mode.base_path,
        round_options => round_options,
        availability_options => availability_options,
        sort_options => sort_options,
        per_options => per_options,
        filter_round => round.as_deref().unwrap_or("all"),
        filter_availability => availability,
        filter_sort => sort,
        filter_q => search.unwrap_or(""),
        current_page => page,
        per_page => per,
        total_count => total_count,
        total_pages_count => total_pages_count,
        has_previous => previous_url.is_some(),
        has_next => next_url.is_some(),
        previous_url => previous_url.unwrap_or_default(),
        next_url => next_url.unwrap_or_default(),
        page_links => page_links,
        daily_tally => daily_tally,
        daily_limit => daily_limit,
        limit_reached => daily_tally >= daily_limit,
    };

    let is_htmx = headers
        .get("HX-Request")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));

    let template_name = if is_htmx {
        "projects/_list_results.html"
    } else {
        "projects/list.html"
    };
    let tmpl = state.templates.get_template(template_name)?;
    Ok(axum::response::Html(tmpl.render(ctx)?).into_response())
}

/// `GET /projects` — list of available projects with filters and pagination.
#[instrument(skip(state, auth, headers))]
pub async fn project_list(
    auth: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<ProjectListQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    render_project_list(
        auth,
        &state,
        query,
        ListMode {
            base_path: "/projects",
            my_projects: false,
            default_availability: "available",
        },
        &headers,
    )
    .await
}

/// `GET /my/projects` — projects managed by the current user.
#[instrument(skip(state, manager, headers))]
pub async fn my_projects(
    manager: ManagerUser,
    State(state): State<AppState>,
    Query(query): Query<ProjectListQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    render_project_list(
        manager.0,
        &state,
        query,
        ListMode {
            base_path: "/my/projects",
            my_projects: true,
            default_availability: "all",
        },
        &headers,
    )
    .await
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

    // Check daily limit; send the user back with a clear, actionable message.
    let daily_limit = SiteSettings::load(&state.db, &state.config.limits)
        .await?
        .daily_pages;
    match check_daily_limit(&state.db, auth.user_id.0, daily_limit).await {
        Ok(()) => {}
        Err(DbError::Domain(dp_core::DomainError::DailyLimitReached { .. })) => {
            return Ok(
                Redirect::to(&format!("/projects/{id}?notice=limit_reached")).into_response(),
            );
        }
        Err(err) => return Err(err.into()),
    }

    // Check if user already holds a page in progress
    if let Some(active) = get_user_active_claim(&state.db, auth.user_id.0).await? {
        return Ok(Redirect::to(&format!(
            "/projects/{}/pages/{}",
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
            Ok(Redirect::to(&format!("/projects/{id}/pages/{}", page.page_number)).into_response())
        }
        None => Ok(Redirect::to(&format!("/projects/{id}?notice=no_pages")).into_response()),
    }
}
