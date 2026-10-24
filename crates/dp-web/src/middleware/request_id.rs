//! Request ID propagation middleware.

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use uuid::Uuid;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Strongly-typed wrapper around a request tracing ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

/// Middleware that extracts or generates an `X-Request-Id` and attaches it to the response.
pub async fn set_request_id(mut req: Request, next: Next) -> Response {
    let id_str = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    req.extensions_mut().insert(RequestId(id_str.clone()));

    let mut res = next.run(req).await;

    if let Ok(hdr) = HeaderValue::from_str(&id_str) {
        res.headers_mut().insert(REQUEST_ID_HEADER, hdr);
    }

    res
}
