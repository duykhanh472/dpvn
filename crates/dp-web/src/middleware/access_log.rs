//! Structured access logging for every request (§4.6, §21.4).
//!
//! One line per request with `request_id`, method, path, status and duration.
//! Health probes are skipped to keep logs quiet.

use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};

use super::request_id::REQUEST_ID_HEADER;

/// Log a structured line after the response is produced.
///
/// Must be layered *outside* [`super::set_request_id`] so the response header
/// is already present when this middleware runs.
pub async fn access_log(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let started = Instant::now();

    let response = next.run(req).await;

    let status = response.status();
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    let request_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");

    if !matches!(path.as_str(), "/healthz" | "/readyz") {
        tracing::info!(
            request_id,
            method = %method,
            path = %path,
            status = status.as_u16(),
            duration_ms,
            "request completed"
        );
    }

    response
}
