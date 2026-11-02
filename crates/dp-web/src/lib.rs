//! HTTP web server, routing, and operational CLI for Distributed Proofreaders.

pub mod app;
pub mod auth;
pub mod config;
pub mod ingest;
pub mod jobs;
pub mod manifest;
pub mod middleware;
pub mod routes;
pub mod settings;

pub use app::{AppError, AppState, create_router, create_template_env, format_vietnam_time};
pub use auth::{AdminUser, AuthUser, Csrf, ManagerUser, OptionalAuthUser, RateLimiter};
pub use config::AppConfig;
pub use manifest::AssetManifest;
pub use settings::SiteSettings;
