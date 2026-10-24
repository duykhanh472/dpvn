//! HTTP web server, routing, and operational CLI for Distributed Proofreaders.

pub mod app;
pub mod auth;
pub mod config;
pub mod manifest;
pub mod middleware;
pub mod routes;

pub use app::{AppError, AppState, create_router, create_template_env};
pub use auth::{AdminUser, AuthUser, Csrf, ManagerUser, OptionalAuthUser, RateLimiter};
pub use config::AppConfig;
pub use manifest::AssetManifest;
