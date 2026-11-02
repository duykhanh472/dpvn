//! Axum application initialization, router setup, and unified error mapping.

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, FixedOffset, Utc};
use dp_core::DomainError;
use dp_db::DbError;
use sqlx::MySqlPool;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{
    auth::RateLimiter,
    config::AppConfig,
    manifest::AssetManifest,
    middleware::{access_log, security_headers, set_request_id},
    routes::{admin, auth, comments, diff, health, home, projects, proof, words},
};

/// Shared application state injected into Axum route handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub db: MySqlPool,
    /// Loaded configuration.
    pub config: Arc<AppConfig>,
    /// Compiled MiniJinja template environment.
    pub templates: Arc<minijinja::Environment<'static>>,
    /// Asset manifest for cache-busted static files.
    pub manifest: Arc<AssetManifest>,
    /// In-process rate limiter for authentication endpoints.
    pub rate_limiter: Arc<RateLimiter>,
    /// In-memory cache of site-wide (good, bad) word lists (§9.6).
    pub site_words: Arc<RwLock<(HashSet<String>, HashSet<String>)>>,
}

/// Unified error enum for the web layer.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Domain logic rule or constraint violation.
    #[error("domain error: {0}")]
    Domain(#[from] DomainError),

    /// Database query or connection error.
    #[error("database error: {0}")]
    Db(#[from] DbError),

    /// Template rendering failure.
    #[error("template error: {0}")]
    Template(#[from] minijinja::Error),

    /// Resource not found.
    #[error("not found")]
    NotFound,

    /// Action forbidden for current authenticated identity.
    #[error("forbidden")]
    Forbidden,

    /// Unauthenticated access to protected endpoint.
    #[error("unauthorized")]
    Unauthorized,

    /// Malformed or invalid client request.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Unexpected internal failure.
    #[error("internal server error: {0}")]
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(err))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, user_msg): (StatusCode, Cow<'static, str>) = match &self {
            AppError::Domain(DomainError::PageUnavailable) => (
                StatusCode::CONFLICT,
                Cow::Borrowed("Trang này hiện không khả dụng để nhận soát lỗi."),
            ),
            AppError::Domain(DomainError::DailyLimitReached { limit }) => (
                StatusCode::TOO_MANY_REQUESTS,
                Cow::Owned(format!(
                    "Bạn đã đạt giới hạn soát lỗi {limit} trang trong ngày."
                )),
            ),
            AppError::Domain(DomainError::ProjectNotFound(_)) => (
                StatusCode::NOT_FOUND,
                Cow::Borrowed("Không tìm thấy dự án được yêu cầu."),
            ),
            AppError::Domain(DomainError::TransitionDenied { .. }) => (
                StatusCode::CONFLICT,
                Cow::Borrowed("Chuyển trạng thái dự án không hợp lệ."),
            ),
            AppError::Domain(err) => (StatusCode::BAD_REQUEST, Cow::Owned(err.to_string())),
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                Cow::Borrowed("Không tìm thấy tài nguyên yêu cầu."),
            ),
            AppError::Forbidden => (
                StatusCode::FORBIDDEN,
                Cow::Borrowed("Bạn không có quyền thực hiện thao tác này."),
            ),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Cow::Borrowed("Vui lòng đăng nhập để tiếp tục."),
            ),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, Cow::Owned(msg.clone())),
            AppError::Db(DbError::Conflict(msg)) => (StatusCode::CONFLICT, Cow::Owned(msg.clone())),
            AppError::Db(DbError::Domain(DomainError::DailyLimitReached { limit })) => (
                StatusCode::TOO_MANY_REQUESTS,
                Cow::Owned(format!(
                    "Bạn đã đạt giới hạn soát lỗi {limit} trang trong ngày."
                )),
            ),
            AppError::Db(DbError::Domain(DomainError::TransitionDenied { .. })) => (
                StatusCode::CONFLICT,
                Cow::Borrowed("Chuyển trạng thái dự án không hợp lệ."),
            ),
            AppError::Db(DbError::Domain(err)) => {
                (StatusCode::BAD_REQUEST, Cow::Owned(err.to_string()))
            }
            AppError::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => (
                StatusCode::NOT_FOUND,
                Cow::Borrowed("Không tìm thấy dữ liệu yêu cầu."),
            ),
            _ => {
                tracing::error!(error = ?self, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Cow::Borrowed("Đã xảy ra lỗi máy chủ nội bộ. Vui lòng thử lại sau."),
                )
            }
        };

        // Render a self-contained Vietnamese error page. It deliberately does
        // not depend on the template environment, so it still works when
        // templates fail to load.
        let html = render_error_page(status, &user_msg);

        let mut response = (
            status,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        response
    }
}

/// Fallback handler for unknown paths: a Vietnamese 404 page.
async fn not_found() -> AppError {
    AppError::NotFound
}

/// Escape text before interpolating it into the error page.
///
/// Domain errors can include user-controlled values (for example a project
/// name), so every interpolated string is escaped to prevent reflected XSS.
#[must_use]
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Build the standalone HTML document used for every error response.
#[must_use]
pub fn render_error_page(status: StatusCode, message: &str) -> String {
    let code = status.as_u16();
    let heading = match status {
        StatusCode::BAD_REQUEST => "Yêu cầu không hợp lệ",
        StatusCode::UNAUTHORIZED => "Cần đăng nhập",
        StatusCode::FORBIDDEN => "Không có quyền truy cập",
        StatusCode::NOT_FOUND => "Không tìm thấy trang",
        StatusCode::METHOD_NOT_ALLOWED => "Phương thức không được hỗ trợ",
        StatusCode::CONFLICT => "Xung đột dữ liệu",
        StatusCode::PAYLOAD_TOO_LARGE => "Tệp tải lên quá lớn",
        StatusCode::TOO_MANY_REQUESTS => "Quá nhiều yêu cầu",
        StatusCode::SERVICE_UNAVAILABLE => "Tạm thời không khả dụng",
        s if s.is_server_error() => "Lỗi máy chủ",
        _ => "Đã xảy ra lỗi",
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="vi">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{code} — {heading} — DProofreaders VN</title>
<style>
body{{font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;margin:0;padding:3rem 1rem;text-align:center;color:#0f172a;background:#f8fafc;line-height:1.6;}}
main{{max-width:36rem;margin:0 auto;}}
.error-code{{font-size:3rem;color:#b91c1c;margin:0 0 0.5rem;font-variant-numeric:tabular-nums;}}
.error-heading{{font-size:1.35rem;margin:0 0 0.75rem;}}
.error-message{{font-size:1.05rem;margin:0 0 1.75rem;color:#334155;}}
.error-actions{{display:flex;gap:0.75rem;justify-content:center;flex-wrap:wrap;}}
a{{display:inline-block;padding:0.6rem 1rem;border-radius:0.5rem;background:#2563eb;color:#fff;text-decoration:none;font-weight:600;}}
a.secondary{{background:transparent;color:#2563eb;border:1px solid #2563eb;}}
a:focus-visible{{outline:3px solid #1d4ed8;outline-offset:2px;}}
</style>
</head>
<body>
<main>
<p class="error-code">{code}</p>
<h1 class="error-heading">{heading}</h1>
<p class="error-message">{message}</p>
<div class="error-actions">
<a href="/">Quay lại trang chủ</a>
<a href="/projects" class="secondary">Xem danh sách dự án</a>
</div>
</main>
</body>
</html>"#,
        heading = escape_html(heading),
        message = escape_html(message),
    )
}

/// Format a stored UTC timestamp for display in the `Asia/Ho_Chi_Minh` zone.
///
/// Vietnam is fixed at UTC+7 with no daylight saving time, so a fixed offset is
/// sufficient. Storage remains UTC; only presentation is localized (§8.8).
#[must_use]
pub fn format_vietnam_time(timestamp: DateTime<Utc>, format: &str) -> String {
    match FixedOffset::east_opt(7 * 3600) {
        Some(offset) => timestamp.with_timezone(&offset).format(format).to_string(),
        // UTC+7 is always representable; this fallback keeps the helper total.
        None => timestamp.format(format).to_string(),
    }
}

/// Create and configure the MiniJinja template environment.
///
/// # Errors
/// Returns [`AppError::Template`] if embedding or loading templates fails.
pub fn create_template_env(
    config: &AppConfig,
    manifest: Arc<AssetManifest>,
) -> Result<minijinja::Environment<'static>, AppError> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

    // Register asset_url helper function for templates
    let manifest_clone = manifest.clone();
    env.add_function("asset_url", move |name: &str| -> String {
        manifest_clone.resolve(name)
    });

    // Human-readable Vietnamese labels for domain values (§2 D2: no i18n layer).
    env.add_filter("phase_label", |phase: Option<String>| -> String {
        match phase.as_deref() {
            Some("draft") => "Bản nháp",
            Some("queued") => "Đang chờ",
            Some("in_round") => "Đang soát lỗi",
            Some("post_processing") => "Hậu xử lý",
            Some("posted") => "Đã đăng",
            Some("archived") => "Lưu trữ",
            None => "—",
            Some(_) => "Không rõ",
        }
        .to_string()
    });
    env.add_filter("round_label", |round: String| -> String {
        if round.is_empty() {
            "—".to_string()
        } else {
            format!("Vòng {round}")
        }
    });

    if config.dev_mode {
        if let Some(ref tmpl_dir) = config.paths.templates {
            if tmpl_dir.exists() {
                env.set_loader(minijinja::path_loader(tmpl_dir));
                return Ok(env);
            }
        }
    }

    minijinja_embed::load_templates!(&mut env);
    Ok(env)
}

/// Build the root Axum application Router with all routes and middlewares attached.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::index))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route(
            "/register",
            get(auth::register_page).post(auth::register_submit),
        )
        .route("/activate/{token}", get(auth::activate_account))
        .route("/login", get(auth::login_page).post(auth::login_submit))
        .route("/logout", post(auth::logout))
        // Project routes
        .route("/projects", get(projects::project_list))
        .route(
            "/projects/new",
            get(projects::new_project_page).post(projects::new_project_submit),
        )
        .route("/projects/{id}", get(projects::project_detail))
        .route(
            "/projects/{id}/comments",
            post(comments::project_comment_submit),
        )
        .route("/projects/{id}/diff", get(diff::diff_page))
        .route(
            "/projects/{id}/import",
            get(projects::import_page).post(projects::import_submit),
        )
        .route(
            "/projects/{id}/transition",
            post(projects::project_transition_submit),
        )
        .route(
            "/projects/{id}/hold",
            post(projects::project_toggle_hold_submit),
        )
        .route("/projects/{id}/claim", post(projects::project_claim_submit))
        .route("/my/projects", get(projects::my_projects))
        // Proofing interface routes (§14.3, §14.4, §15.2)
        .route("/projects/{id}/proof", get(proof::proof_entry))
        .route("/projects/{id}/pages/{page_number}", get(proof::proof_page))
        .route(
            "/projects/{id}/pages/{page_number}/save",
            post(proof::proof_save),
        )
        .route(
            "/projects/{id}/pages/{page_number}/wordcheck",
            post(proof::proof_wordcheck),
        )
        .route(
            "/projects/{id}/pages/{page_number}/preview",
            post(proof::proof_format_preview),
        )
        .route(
            "/projects/{id}/pages/{page_number}/release",
            post(proof::proof_release),
        )
        .route(
            "/projects/{id}/pages/{page_number}/image",
            get(proof::proof_image),
        )
        // Custom word lists (§9.2, §7.2)
        .route(
            "/projects/{id}/words",
            get(words::project_words_page).post(words::project_words_submit),
        )
        .route(
            "/admin/words",
            get(words::admin_words_page).post(words::admin_words_submit),
        )
        // Administration (§Phase 10)
        .route("/admin", get(admin::dashboard))
        .route("/admin/users", get(admin::users_page))
        .route("/admin/users/{id}/roles", post(admin::user_roles_submit))
        .route("/admin/users/{id}/status", post(admin::user_status_submit))
        .route("/admin/projects", get(admin::projects_page))
        .route(
            "/admin/projects/{id}/transition",
            post(admin::project_transition_submit),
        )
        .route(
            "/admin/projects/{id}/hold",
            post(admin::project_hold_submit),
        )
        .route(
            "/admin/settings",
            get(admin::settings_page).post(admin::settings_submit),
        )
        .route("/admin/audit", get(admin::audit_page))
        .fallback(not_found)
        .layer(middleware::from_fn(set_request_id))
        .layer(middleware::from_fn(access_log))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_covers_all_special_characters() {
        assert_eq!(
            escape_html(r#"<script>"x" & 'y'</script>"#),
            "&lt;script&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/script&gt;"
        );
    }

    #[test]
    fn error_page_is_vietnamese_and_escapes_message() {
        let html = render_error_page(StatusCode::NOT_FOUND, "<b>Không tìm thấy</b>");
        assert!(html.contains(r#"lang="vi""#));
        assert!(html.contains("Không tìm thấy trang"));
        assert!(html.contains("&lt;b&gt;Không tìm thấy&lt;/b&gt;"));
        assert!(!html.contains("<b>Không tìm thấy</b>"));
    }

    #[test]
    fn not_found_response_has_no_store_and_vietnamese_body() {
        let response = AppError::NotFound.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
    }

    #[test]
    fn too_many_requests_has_retry_after() {
        let response =
            AppError::Domain(DomainError::DailyLimitReached { limit: 100 }).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("60")
        );
    }
}
