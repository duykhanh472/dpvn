//! Administrative routes: users, project oversight, settings, and audit log (Phase 10).
//!
//! Every handler is guarded by the [`AdminUser`] extractor. Mutating handlers
//! verify the session CSRF token, require explicit confirmation for destructive
//! operations, and append an entry to `admin_audit_log` on success.

use std::str::FromStr;

use axum::{
    Form,
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use dp_core::{ActorRole, Phase, RoundsConfig, UserId};
use dp_db::queries::{
    admin_audit::{NewAdminAction, count_admin_actions, list_admin_actions, record_admin_action},
    project_events::{count_all_project_events, list_all_project_events_with_actors},
    projects::{
        ProjectListFilter, count_projects_filtered, get_project_by_id, list_projects_filtered,
        toggle_project_hold, transition_project,
    },
    settings::{KEY_CLAIM_TIMEOUT_HOURS, KEY_DAILY_PAGES, KEY_REGISTRATION_OPEN, upsert_settings},
    users::{
        AdminUserFilter, count_active_admins, count_users_for_admin, get_user_by_id,
        grant_user_role, list_users_for_admin, revoke_user_role_guarded, set_user_status_guarded,
    },
};
use minijinja::context;
use serde::Deserialize;
use tracing::instrument;

use crate::{
    AppError, AppState,
    auth::{AdminUser, verify_csrf_token},
    format_vietnam_time,
    settings::SiteSettings,
};

/// Supported page sizes for admin lists.
const PER_PAGE_OPTIONS: [u32; 3] = [25, 50, 100];
/// Default page size.
const DEFAULT_PER_PAGE: u32 = 25;
/// Hard cap on page size.
const MAX_PER_PAGE: u32 = 100;

/// Assignable roles with their Vietnamese labels.
const ROLES: [(&str, &str); 4] = [
    ("proofer", "Người soát lỗi"),
    ("pm", "Quản lý dự án"),
    ("pp", "Hậu xử lý"),
    ("admin", "Quản trị viên"),
];

/// Account statuses with their Vietnamese labels.
const STATUSES: [(&str, &str); 3] = [
    ("pending", "Chờ kích hoạt"),
    ("active", "Hoạt động"),
    ("disabled", "Đã vô hiệu hóa"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

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

/// Require the explicit confirmation field used by destructive forms.
fn require_confirm(value: Option<&str>) -> Result<(), AppError> {
    match value.map(str::trim) {
        Some("1" | "true" | "on" | "yes") => Ok(()),
        _ => Err(AppError::BadRequest(
            "Vui lòng xác nhận thao tác phá hủy này.".to_string(),
        )),
    }
}

/// Append an administrative audit entry.
async fn audit(
    state: &AppState,
    admin: &AdminUser,
    action: &str,
    target_type: &str,
    target_id: Option<u64>,
    detail: String,
) -> Result<(), AppError> {
    record_admin_action(
        &state.db,
        &NewAdminAction {
            actor_id: admin.0.user_id.0,
            actor_username: &admin.0.username,
            action,
            target_type,
            target_id,
            detail: &detail,
        },
    )
    .await?;
    Ok(())
}

/// Translate a stored action code into a Vietnamese label for the audit view.
fn admin_action_label(action: &str) -> String {
    match action {
        "user.role.grant" => "Cấp quyền",
        "user.role.revoke" => "Thu hồi quyền",
        "user.status" => "Đổi trạng thái tài khoản",
        "settings.update" => "Cập nhật cài đặt",
        "project.transition" => "Chuyển trạng thái dự án",
        "project.hold" => "Bật/tắt tạm dừng dự án",
        "site_word.add" => "Thêm từ điển hệ thống",
        "site_word.delete" => "Xóa từ điển hệ thống",
        other => other,
    }
    .to_string()
}

/// Compute pagination metadata for a list.
fn pagination(total: i64, per: u32, requested: u32) -> (u32, u32, u32) {
    let total_pages = if total == 0 {
        1
    } else {
        ((total + i64::from(per) - 1) / i64::from(per)) as u32
    };
    let page = requested.max(1).min(total_pages);
    let offset = (page - 1) * per;
    (page, total_pages, offset)
}

fn role_options(selected: Option<&str>) -> Vec<minijinja::Value> {
    ROLES
        .iter()
        .map(|(value, label)| {
            context! {
                value => *value,
                label => *label,
                selected => selected == Some(*value),
            }
        })
        .collect()
}

fn status_options(selected: Option<&str>) -> Vec<minijinja::Value> {
    STATUSES
        .iter()
        .map(|(value, label)| {
            context! {
                value => *value,
                label => *label,
                selected => selected == Some(*value),
            }
        })
        .collect()
}

fn per_options(selected: u32) -> Vec<minijinja::Value> {
    PER_PAGE_OPTIONS
        .iter()
        .map(|value| {
            context! {
                value => *value,
                label => format!("{value} / trang"),
                selected => *value == selected,
            }
        })
        .collect()
}

fn phase_options(selected: Option<&str>) -> Vec<minijinja::Value> {
    let label_for = |phase: &str| -> &'static str {
        match phase {
            "draft" => "Bản nháp",
            "queued" => "Đang chờ",
            "in_round" => "Đang soát lỗi",
            "post_processing" => "Hậu xử lý",
            "posted" => "Đã đăng",
            "archived" => "Lưu trữ",
            _ => "Không rõ",
        }
    };

    std::iter::once(("all", "Tất cả trạng thái"))
        .chain(
            [
                Phase::Draft,
                Phase::Queued,
                Phase::InRound,
                Phase::PostProcessing,
                Phase::Posted,
                Phase::Archived,
            ]
            .into_iter()
            .map(|phase| (phase.as_str(), label_for(phase.as_str()))),
        )
        .map(|(value, label)| {
            context! {
                value => value,
                label => label,
                selected => selected == Some(value) || (selected.is_none() && value == "all"),
            }
        })
        .collect()
}

/// Admin transition targets permitted by the domain state machine (§5.3).
fn allowed_targets(phase: &str) -> Vec<(&'static str, &'static str)> {
    match phase {
        "draft" => vec![("queued", "Đưa vào hàng chờ")],
        "queued" => vec![
            ("in_round", "Bắt đầu vòng P1"),
            ("draft", "Trả về bản nháp"),
        ],
        "in_round" => vec![("draft", "Hoàn tác về bản nháp")],
        "post_processing" => vec![("posted", "Đánh dấu đã đăng")],
        "posted" => vec![("archived", "Lưu trữ")],
        _ => Vec::new(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dashboard
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /admin` — administrative overview with shortcuts and recent activity.
#[instrument(skip(state, admin))]
pub async fn dashboard(
    admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let total_users = count_users_for_admin(&state.db, &AdminUserFilter::default()).await?;
    let active_users = count_users_for_admin(
        &state.db,
        &AdminUserFilter {
            status: Some("active"),
            ..AdminUserFilter::default()
        },
    )
    .await?;
    let pending_users = count_users_for_admin(
        &state.db,
        &AdminUserFilter {
            status: Some("pending"),
            ..AdminUserFilter::default()
        },
    )
    .await?;
    let active_admins = count_active_admins(&state.db).await?;
    let total_projects = count_projects_filtered(
        &state.db,
        &ProjectListFilter {
            sort: "recent",
            ..ProjectListFilter::default()
        },
    )
    .await?;
    let admin_action_count = count_admin_actions(&state.db).await?;
    let project_event_count = count_all_project_events(&state.db).await?;

    let recent_actions = list_admin_actions(&state.db, 5, 0).await?;
    let recent_action_ctx: Vec<_> = recent_actions
        .iter()
        .map(|action| {
            context! {
                id => action.id,
                action_label => admin_action_label(&action.action),
                actor_username => action.actor_username.clone(),
                detail => action.detail.clone(),
                created_at_label => format_vietnam_time(action.created_at, "%d/%m/%Y %H:%M"),
            }
        })
        .collect();

    let recent_events = list_all_project_events_with_actors(&state.db, 5, 0).await?;
    let recent_event_ctx: Vec<_> = recent_events
        .iter()
        .map(|event| {
            context! {
                id => event.id,
                project_id => event.project_id,
                project_name => event.project_name.clone(),
                from_phase => event.from_phase.clone(),
                to_phase => event.to_phase.clone(),
                actor_username => event.actor_username.clone().unwrap_or_else(|| "Hệ thống".to_string()),
                note => event.note.clone(),
                created_at_label => format_vietnam_time(event.created_at, "%d/%m/%Y %H:%M"),
            }
        })
        .collect();

    let tmpl = state.templates.get_template("admin/index.html")?;
    let rendered = tmpl.render(context! {
        user => context! {
            username => admin.0.username.clone(),
            roles => admin.0.roles.clone(),
        },
        csrf_token => admin.0.csrf_token.clone(),
        admin_tab => "dashboard",
        total_users => total_users,
        active_users => active_users,
        pending_users => pending_users,
        active_admins => active_admins,
        total_projects => total_projects,
        admin_action_count => admin_action_count,
        project_event_count => project_event_count,
        recent_actions => recent_action_ctx,
        recent_events => recent_event_ctx,
    })?;

    Ok(Html(rendered).into_response())
}

// ─────────────────────────────────────────────────────────────────────────────
// Users and roles
// ─────────────────────────────────────────────────────────────────────────────

/// Query parameters for the admin user list.
#[derive(Debug, Default, Deserialize)]
pub struct AdminUserQuery {
    /// Substring search over username, email, and display name.
    pub q: Option<String>,
    /// Role filter.
    pub role: Option<String>,
    /// Status filter.
    pub status: Option<String>,
    /// Page number, 1-based.
    pub page: Option<u32>,
    /// Page size, capped at [`MAX_PER_PAGE`].
    pub per: Option<u32>,
}

fn users_url(search: &str, role: &str, status: &str, page: u32, per: u32) -> String {
    format!(
        "/admin/users?q={}&role={}&status={}&page={page}&per={per}",
        encode_component(search),
        encode_component(role),
        encode_component(status),
    )
}

/// `GET /admin/users` — user list with role and status management.
#[instrument(skip(state, admin))]
pub async fn users_page(
    admin: AdminUser,
    State(state): State<AppState>,
    Query(query): Query<AdminUserQuery>,
) -> Result<Response, AppError> {
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let role = query
        .role
        .as_deref()
        .filter(|value| ROLES.iter().any(|(candidate, _)| candidate == value));
    let status = query
        .status
        .as_deref()
        .filter(|value| STATUSES.iter().any(|(candidate, _)| candidate == value));

    let per = query.per.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
    let requested_page = query.page.unwrap_or(1);

    let count_filter = AdminUserFilter {
        search,
        role,
        status,
        limit: 0,
        offset: 0,
    };
    let total_count = count_users_for_admin(&state.db, &count_filter).await?;
    let (page, total_pages, offset) = pagination(total_count, per, requested_page);

    let users = list_users_for_admin(
        &state.db,
        &AdminUserFilter {
            limit: per,
            offset,
            ..count_filter
        },
    )
    .await?;

    let user_ctx: Vec<_> = users
        .iter()
        .map(|user| {
            let roles: Vec<String> = if user.roles.is_empty() {
                Vec::new()
            } else {
                user.roles.split(',').map(str::to_string).collect()
            };
            context! {
                id => user.id,
                username => user.username.clone(),
                email => user.email.clone(),
                real_name => user.real_name.clone(),
                status => user.status.clone(),
                roles => roles,
                is_self => user.id == admin.0.user_id.0,
                created_at_label => format_vietnam_time(user.created_at, "%d/%m/%Y %H:%M"),
                last_login_label => user.last_login_at
                    .map(|ts| format_vietnam_time(ts, "%d/%m/%Y %H:%M"))
                    .unwrap_or_else(|| "Chưa đăng nhập".to_string()),
            }
        })
        .collect();

    let search_value = search.unwrap_or("");
    let role_value = role.unwrap_or("");
    let status_value = status.unwrap_or("");
    let previous_url =
        (page > 1).then(|| users_url(search_value, role_value, status_value, page - 1, per));
    let next_url = (page < total_pages)
        .then(|| users_url(search_value, role_value, status_value, page + 1, per));

    let tmpl = state.templates.get_template("admin/users.html")?;
    let rendered = tmpl.render(context! {
        user => context! {
            username => admin.0.username.clone(),
            roles => admin.0.roles.clone(),
        },
        csrf_token => admin.0.csrf_token.clone(),
        admin_tab => "users",
        users => user_ctx,
        total_count => total_count,
        current_page => page,
        total_pages => total_pages,
        per_page => per,
        per_options => per_options(per),
        filter_q => search_value,
        filter_role => role_value,
        filter_status => status_value,
        role_options => role_options(role),
        status_options => status_options(status),
        all_roles => role_options(None),
        has_previous => previous_url.is_some(),
        has_next => next_url.is_some(),
        previous_url => previous_url.unwrap_or_default(),
        next_url => next_url.unwrap_or_default(),
    })?;

    Ok(Html(rendered).into_response())
}

/// Form for granting or revoking a role.
#[derive(Debug, Deserialize)]
pub struct RoleChangeForm {
    /// `grant` or `revoke`.
    pub action: String,
    /// Role name.
    pub role: String,
    /// Explicit confirmation for revocations.
    #[serde(default)]
    pub confirm: Option<String>,
    /// Session CSRF token.
    pub csrf_token: String,
}

/// `POST /admin/users/{id}/roles` — grant or revoke a role.
#[instrument(skip(state, admin, form), fields(user_id = id))]
pub async fn user_roles_submit(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<RoleChangeForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &admin.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let role = form.role.trim();
    if !ROLES.iter().any(|(candidate, _)| *candidate == role) {
        return Err(AppError::BadRequest("Quyền không hợp lệ.".to_string()));
    }

    let target = get_user_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    match form.action.as_str() {
        "grant" => {
            let changed = grant_user_role(&state.db, id, role).await?;
            if changed {
                audit(
                    &state,
                    &admin,
                    "user.role.grant",
                    "user",
                    Some(id),
                    format!("cấp quyền '{role}' cho {}", target.username),
                )
                .await?;
            }
        }
        "revoke" => {
            require_confirm(form.confirm.as_deref())?;
            let changed = revoke_user_role_guarded(&state.db, id, role).await?;
            if changed {
                audit(
                    &state,
                    &admin,
                    "user.role.revoke",
                    "user",
                    Some(id),
                    format!("thu hồi quyền '{role}' của {}", target.username),
                )
                .await?;
            }
        }
        _ => return Err(AppError::BadRequest("Thao tác không hợp lệ.".to_string())),
    }

    Ok(Redirect::to("/admin/users").into_response())
}

/// Form for enabling or disabling an account.
#[derive(Debug, Deserialize)]
pub struct StatusChangeForm {
    /// `active` or `disabled`.
    pub status: String,
    /// Explicit confirmation when disabling.
    #[serde(default)]
    pub confirm: Option<String>,
    /// Session CSRF token.
    pub csrf_token: String,
}

/// `POST /admin/users/{id}/status` — enable or disable an account.
#[instrument(skip(state, admin, form), fields(user_id = id))]
pub async fn user_status_submit(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<StatusChangeForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &admin.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let status = form.status.trim();
    if !matches!(status, "active" | "disabled") {
        return Err(AppError::BadRequest(
            "Trạng thái tài khoản không hợp lệ.".to_string(),
        ));
    }

    if id == admin.0.user_id.0 && status == "disabled" {
        return Err(AppError::BadRequest(
            "Bạn không thể tự vô hiệu hóa tài khoản của mình.".to_string(),
        ));
    }

    if status == "disabled" {
        require_confirm(form.confirm.as_deref())?;
    }

    let target = get_user_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let changed = set_user_status_guarded(&state.db, id, status).await?;
    if changed {
        let action = if status == "active" {
            "kích hoạt"
        } else {
            "vô hiệu hóa"
        };
        audit(
            &state,
            &admin,
            "user.status",
            "user",
            Some(id),
            format!("{action} tài khoản {}", target.username),
        )
        .await?;
    }

    Ok(Redirect::to("/admin/users").into_response())
}

// ─────────────────────────────────────────────────────────────────────────────
// Project oversight
// ─────────────────────────────────────────────────────────────────────────────

/// Query parameters for the admin project list.
#[derive(Debug, Default, Deserialize)]
pub struct AdminProjectQuery {
    /// Substring search over project name and author.
    pub q: Option<String>,
    /// Exact phase filter.
    pub phase: Option<String>,
    /// Page number, 1-based.
    pub page: Option<u32>,
    /// Page size, capped at [`MAX_PER_PAGE`].
    pub per: Option<u32>,
}

fn projects_url(search: &str, phase: &str, page: u32, per: u32) -> String {
    format!(
        "/admin/projects?q={}&phase={}&page={page}&per={per}",
        encode_component(search),
        encode_component(phase),
    )
}

/// `GET /admin/projects` — project oversight with force transition and hold controls.
#[instrument(skip(state, admin))]
pub async fn projects_page(
    admin: AdminUser,
    State(state): State<AppState>,
    Query(query): Query<AdminProjectQuery>,
) -> Result<Response, AppError> {
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(dp_core::viet::normalize);
    let phase = query
        .phase
        .as_deref()
        .filter(|value| Phase::from_str(value).is_ok());

    let per = query.per.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
    let requested_page = query.page.unwrap_or(1);

    let count_filter = ProjectListFilter {
        manager_id: None,
        round: None,
        phase,
        availability: None,
        search: search.as_deref(),
        sort: "recent",
        limit: 0,
        offset: 0,
    };
    let total_count = count_projects_filtered(&state.db, &count_filter).await?;
    let (page, total_pages, offset) = pagination(total_count, per, requested_page);

    let projects = list_projects_filtered(
        &state.db,
        &ProjectListFilter {
            limit: per,
            offset,
            ..count_filter
        },
    )
    .await?;

    let project_ctx: Vec<_> = projects
        .iter()
        .map(|project| {
            let targets: Vec<_> = allowed_targets(&project.phase)
                .into_iter()
                .map(|(value, label)| context! { value => value, label => label })
                .collect();
            context! {
                id => project.id,
                name => project.name.clone(),
                author => project.author.clone(),
                phase => project.phase.clone(),
                current_round => project.current_round.clone(),
                on_hold => project.on_hold,
                manager_username => project.manager_username.clone(),
                total_pages => project.total_pages,
                available_pages => project.available_pages,
                in_progress_pages => project.in_progress_pages,
                done_pages => project.done_pages,
                updated_at_label => format_vietnam_time(project.updated_at, "%d/%m/%Y %H:%M"),
                targets => targets,
                can_transition => !allowed_targets(&project.phase).is_empty(),
            }
        })
        .collect();

    let search_value = search.as_deref().unwrap_or("");
    let phase_value = phase.unwrap_or("");
    let previous_url = (page > 1).then(|| projects_url(search_value, phase_value, page - 1, per));
    let next_url =
        (page < total_pages).then(|| projects_url(search_value, phase_value, page + 1, per));

    let tmpl = state.templates.get_template("admin/projects.html")?;
    let rendered = tmpl.render(context! {
        user => context! {
            username => admin.0.username.clone(),
            roles => admin.0.roles.clone(),
        },
        csrf_token => admin.0.csrf_token.clone(),
        admin_tab => "projects",
        projects => project_ctx,
        total_count => total_count,
        current_page => page,
        total_pages => total_pages,
        per_page => per,
        per_options => per_options(per),
        filter_q => search_value,
        filter_phase => phase_value,
        phase_options => phase_options(phase),
        has_previous => previous_url.is_some(),
        has_next => next_url.is_some(),
        previous_url => previous_url.unwrap_or_default(),
        next_url => next_url.unwrap_or_default(),
    })?;

    Ok(Html(rendered).into_response())
}

/// Form for an admin-forced project transition.
#[derive(Debug, Deserialize)]
pub struct AdminTransitionForm {
    /// Target phase.
    pub target_phase: String,
    /// Explicit confirmation.
    #[serde(default)]
    pub confirm: Option<String>,
    /// Session CSRF token.
    pub csrf_token: String,
}

/// `POST /admin/projects/{id}/transition` — force a project state transition.
#[instrument(skip(state, admin, form), fields(project_id = id))]
pub async fn project_transition_submit(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<AdminTransitionForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &admin.0.csrf_token) {
        return Err(AppError::Forbidden);
    }
    require_confirm(form.confirm.as_deref())?;

    let target_phase = Phase::from_str(form.target_phase.trim())
        .map_err(|e| AppError::BadRequest(format!("Trạng thái không hợp lệ: {e}")))?;

    let project = get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let rounds = RoundsConfig::default_embedded();
    transition_project(
        &state.db,
        id,
        target_phase,
        ActorRole::Admin,
        Some(UserId::new(admin.0.user_id.0)),
        &rounds,
    )
    .await?;

    audit(
        &state,
        &admin,
        "project.transition",
        "project",
        Some(id),
        format!(
            "chuyển dự án '{}' từ {} sang {}",
            project.name, project.phase, target_phase
        ),
    )
    .await?;

    Ok(Redirect::to("/admin/projects").into_response())
}

/// Form for toggling a project's hold flag.
#[derive(Debug, Deserialize)]
pub struct AdminHoldForm {
    /// Explicit confirmation when pausing a project.
    #[serde(default)]
    pub confirm: Option<String>,
    /// Session CSRF token.
    pub csrf_token: String,
}

/// `POST /admin/projects/{id}/hold` — pause or resume a project.
#[instrument(skip(state, admin, form), fields(project_id = id))]
pub async fn project_hold_submit(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(form): Form<AdminHoldForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &admin.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let project = get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Pausing a live project is destructive for volunteers; resuming is not.
    if project.on_hold == 0 {
        require_confirm(form.confirm.as_deref())?;
    }

    let now_on_hold = toggle_project_hold(
        &state.db,
        id,
        ActorRole::Admin,
        Some(UserId::new(admin.0.user_id.0)),
    )
    .await?;

    let state_label = if now_on_hold {
        "tạm dừng"
    } else {
        "kích hoạt lại"
    };
    audit(
        &state,
        &admin,
        "project.hold",
        "project",
        Some(id),
        format!("{state_label} dự án '{}'", project.name),
    )
    .await?;

    Ok(Redirect::to("/admin/projects").into_response())
}

// ─────────────────────────────────────────────────────────────────────────────
// Site settings
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /admin/settings` — runtime settings form.
#[instrument(skip(state, admin))]
pub async fn settings_page(
    admin: AdminUser,
    State(state): State<AppState>,
    Query(query): Query<SettingsQuery>,
) -> Result<Response, AppError> {
    let site_settings = SiteSettings::load(&state.db, &state.config.limits).await?;

    let tmpl = state.templates.get_template("admin/settings.html")?;
    let rendered = tmpl.render(context! {
        user => context! {
            username => admin.0.username.clone(),
            roles => admin.0.roles.clone(),
        },
        csrf_token => admin.0.csrf_token.clone(),
        admin_tab => "settings",
        registration_open => site_settings.registration_open,
        daily_pages => site_settings.daily_pages,
        claim_timeout_hours => site_settings.claim_timeout_hours,
        config_daily_pages => state.config.limits.daily_pages,
        config_claim_timeout_hours => state.config.limits.claim_timeout_hours,
        saved => query.saved.as_deref() == Some("1"),
    })?;

    Ok(Html(rendered).into_response())
}

/// Query parameters for the settings page.
#[derive(Debug, Default, Deserialize)]
pub struct SettingsQuery {
    /// `1` when the last save succeeded.
    pub saved: Option<String>,
}

/// Form for updating runtime settings.
#[derive(Debug, Deserialize)]
pub struct SettingsForm {
    /// Checkbox: present when registration is open.
    #[serde(default)]
    pub registration_open: Option<String>,
    /// Daily proofing page limit.
    pub daily_pages: String,
    /// Claim timeout in hours.
    pub claim_timeout_hours: String,
    /// Session CSRF token.
    pub csrf_token: String,
}

/// `POST /admin/settings` — persist runtime settings.
#[instrument(skip(state, admin, form))]
pub async fn settings_submit(
    admin: AdminUser,
    State(state): State<AppState>,
    Form(form): Form<SettingsForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &admin.0.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let daily_pages = form
        .daily_pages
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| (1..=10_000).contains(value))
        .ok_or_else(|| {
            AppError::BadRequest("Giới hạn trang mỗi ngày phải từ 1 đến 10.000.".to_string())
        })?;
    let claim_timeout_hours = form
        .claim_timeout_hours
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| (1..=8_760).contains(value))
        .ok_or_else(|| {
            AppError::BadRequest("Thời gian giữ trang phải từ 1 đến 8.760 giờ.".to_string())
        })?;
    let registration_open = form
        .registration_open
        .as_deref()
        .is_some_and(|value| matches!(value, "1" | "on" | "true" | "yes"));

    upsert_settings(
        &state.db,
        &[
            (
                KEY_REGISTRATION_OPEN,
                if registration_open { "1" } else { "0" }.to_string(),
            ),
            (KEY_DAILY_PAGES, daily_pages.to_string()),
            (KEY_CLAIM_TIMEOUT_HOURS, claim_timeout_hours.to_string()),
        ],
    )
    .await?;

    audit(
        &state,
        &admin,
        "settings.update",
        "settings",
        None,
        format!(
            "registration_open={}; daily_pages={daily_pages}; claim_timeout_hours={claim_timeout_hours}",
            if registration_open { "1" } else { "0" }
        ),
    )
    .await?;

    Ok(Redirect::to("/admin/settings?saved=1").into_response())
}

// ─────────────────────────────────────────────────────────────────────────────
// Audit log
// ─────────────────────────────────────────────────────────────────────────────

/// Query parameters for the audit log.
#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    /// `admin` (default) or `projects`.
    pub tab: Option<String>,
    /// Page number, 1-based.
    pub page: Option<u32>,
    /// Page size, capped at [`MAX_PER_PAGE`].
    pub per: Option<u32>,
}

fn audit_url(tab: &str, page: u32, per: u32) -> String {
    format!(
        "/admin/audit?tab={}&page={page}&per={per}",
        encode_component(tab)
    )
}

/// `GET /admin/audit` — administrative and project audit trail.
#[instrument(skip(state, admin))]
pub async fn audit_page(
    admin: AdminUser,
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Result<Response, AppError> {
    let tab = match query.tab.as_deref() {
        Some("projects") => "projects",
        _ => "admin",
    };
    let per = query.per.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
    let requested_page = query.page.unwrap_or(1);

    let admin_count = count_admin_actions(&state.db).await?;
    let project_count = count_all_project_events(&state.db).await?;
    let total_count = if tab == "projects" {
        project_count
    } else {
        admin_count
    };
    let (page, total_pages, offset) = pagination(total_count, per, requested_page);

    let (actions, events) = if tab == "projects" {
        let events = list_all_project_events_with_actors(&state.db, per, offset).await?;
        (Vec::new(), events)
    } else {
        let actions = list_admin_actions(&state.db, per, offset).await?;
        (actions, Vec::new())
    };

    let action_ctx: Vec<_> = actions
        .iter()
        .map(|action| {
            context! {
                id => action.id,
                action => action.action.clone(),
                action_label => admin_action_label(&action.action),
                actor_username => action.actor_username.clone(),
                target_type => action.target_type.clone(),
                target_id => action.target_id,
                detail => action.detail.clone(),
                created_at_label => format_vietnam_time(action.created_at, "%d/%m/%Y %H:%M"),
            }
        })
        .collect();

    let event_ctx: Vec<_> = events
        .iter()
        .map(|event| {
            context! {
                id => event.id,
                project_id => event.project_id,
                project_name => event.project_name.clone(),
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

    let previous_url = (page > 1).then(|| audit_url(tab, page - 1, per));
    let next_url = (page < total_pages).then(|| audit_url(tab, page + 1, per));

    let tmpl = state.templates.get_template("admin/audit.html")?;
    let rendered = tmpl.render(context! {
        user => context! {
            username => admin.0.username.clone(),
            roles => admin.0.roles.clone(),
        },
        csrf_token => admin.0.csrf_token.clone(),
        admin_tab => "audit",
        tab => tab,
        actions => action_ctx,
        events => event_ctx,
        admin_count => admin_count,
        project_count => project_count,
        total_count => total_count,
        current_page => page,
        total_pages => total_pages,
        per_page => per,
        has_previous => previous_url.is_some(),
        has_next => next_url.is_some(),
        previous_url => previous_url.unwrap_or_default(),
        next_url => next_url.unwrap_or_default(),
        admin_tab_url => audit_url("admin", 1, per),
        projects_tab_url => audit_url("projects", 1, per),
    })?;

    Ok(Html(rendered).into_response())
}
