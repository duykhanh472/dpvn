//! Health and readiness probe endpoints.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::app::AppState;

/// Basic liveness probe that pings the MySQL database pool.
pub async fn healthz(State(state): State<AppState>) -> Response {
    match dp_db::ping(&state.db).await {
        Ok(()) => (StatusCode::OK, "OK").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "healthz ping failed");
            (StatusCode::SERVICE_UNAVAILABLE, "DATABASE_UNAVAILABLE").into_response()
        }
    }
}

/// Readiness probe that verifies connectivity and required database schema readiness.
pub async fn readyz(State(state): State<AppState>) -> Response {
    match dp_db::check_ready(&state.db).await {
        Ok(true) => (StatusCode::OK, "READY").into_response(),
        Ok(false) => (StatusCode::SERVICE_UNAVAILABLE, "MIGRATIONS_PENDING").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "readyz readiness check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "DATABASE_ERROR").into_response()
        }
    }
}
