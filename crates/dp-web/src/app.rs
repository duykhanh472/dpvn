//! Axum application initialization, router setup, and unified error mapping.

use axum::{
    Router,
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use dp_core::DomainError;
use dp_db::DbError;
use sqlx::MySqlPool;
use std::borrow::Cow;
use std::sync::Arc;

use crate::{
    auth::RateLimiter,
    config::AppConfig,
    manifest::AssetManifest,
    middleware::set_request_id,
    routes::{auth, health, home},
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

        // Render clean HTML error page
        let html = format!(
            r#"<!DOCTYPE html><html lang="vi"><head><meta charset="utf-8"><title>Lỗi {code} — DProofreaders</title><style>body{{font-family:system-ui,sans-serif;padding:3rem 1rem;text-align:center;color:#333;background:#f9fafb;}}h1{{font-size:2.5rem;color:#dc2626;margin-bottom:0.5rem;}}p{{font-size:1.125rem;margin-bottom:1.5rem;}}a{{color:#2563eb;text-decoration:none;font-weight:500;}}a:hover{{text-decoration:underline;}}</style></head><body><h1>{code}</h1><p>{msg}</p><p><a href="/">Quay lại trang chủ</a></p></body></html>"#,
            code = status.as_u16(),
            msg = user_msg
        );

        (
            status,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response()
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
        .layer(middleware::from_fn(set_request_id))
        .with_state(state)
}
