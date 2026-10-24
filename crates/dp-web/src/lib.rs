//! HTTP web server, routing, and operational CLI for Distributed Proofreaders.

pub mod app;
pub mod config;
pub mod manifest;
pub mod middleware;
pub mod routes;

pub use app::{AppError, AppState, create_router, create_template_env};
pub use config::AppConfig;
pub use manifest::AssetManifest;
