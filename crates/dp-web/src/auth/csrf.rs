//! CSRF token generation and constant-time verification.

use subtle::ConstantTimeEq;

use crate::auth::session::generate_token;

pub const CSRF_HEADER_NAME: &str = "x-csrf-token";

/// Generate a new random CSRF token (43 chars base64url).
#[must_use]
pub fn generate_csrf_token() -> String {
    generate_token()
}

/// Verify a submitted CSRF token against the session's CSRF token using constant-time comparison.
#[must_use]
pub fn verify_csrf_token(submitted: &str, expected: &str) -> bool {
    if submitted.len() != expected.len() {
        return false;
    }
    submitted.as_bytes().ct_eq(expected.as_bytes()).into()
}
