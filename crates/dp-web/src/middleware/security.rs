//! Security response headers applied to every response.
//!
//! nginx sets the same headers at the edge, but the application also sets them
//! so that deployments behind another proxy, and the integration test suite,
//! still get a safe baseline. The policy deliberately allows **no** inline or
//! eval'd JavaScript: every script must be served from `/static/`.

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// Content-Security-Policy used by the application and mirrored in nginx.
///
/// `style-src` keeps `'unsafe-inline'` because templates use a small number of
/// inline `style` attributes; `script-src` does not, which is the important
/// guarantee (§21.2).
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     object-src 'none'; \
     img-src 'self' data:; \
     script-src 'self'; \
     style-src 'self' 'unsafe-inline'; \
     font-src 'self'; \
     connect-src 'self'";

/// Add defense-in-depth security headers to the response.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );

    response
}
