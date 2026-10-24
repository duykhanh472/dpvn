//! Session token issuance, SHA-256 hashing, and cookie building.

use base64::prelude::*;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const SESSION_COOKIE_NAME: &str = "dp_session";
pub const SESSION_MAX_AGE_SECS: u32 = 30 * 24 * 60 * 60; // 30 days

/// Generate a cryptographically secure 32-byte random token encoded as URL-safe base64 (43 chars).
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    BASE64_URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute the SHA-256 digest of a session token string for database storage and lookup.
#[must_use]
pub fn hash_token(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

/// Build the `Set-Cookie` header value for an authenticated session.
#[must_use]
pub fn build_session_cookie(token: &str, is_secure: bool) -> String {
    if is_secure {
        format!(
            "{SESSION_COOKIE_NAME}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={SESSION_MAX_AGE_SECS}"
        )
    } else {
        format!(
            "{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_MAX_AGE_SECS}"
        )
    }
}

/// Build a `Set-Cookie` header value to expire and clear the session cookie.
#[must_use]
pub fn build_expired_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}
