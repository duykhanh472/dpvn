//! Argon2id password hashing and constant-time verification.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use dp_core::DomainError;

use crate::app::AppError;

/// Create an Argon2 instance configured according to OWASP baseline (§12.1).
///
/// - Memory: 19,456 KiB (19 MiB)
/// - Iterations: 2
/// - Parallelism: 1
/// - Output: 32 bytes
fn configured_argon2() -> Result<Argon2<'static>, AppError> {
    let params = Params::new(19456, 2, 1, Some(32))
        .map_err(|e| DomainError::InvalidInput(format!("invalid argon2 parameters: {e}")))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hash a password using Argon2id inside `tokio::task::spawn_blocking`.
///
/// # Errors
/// Returns [`AppError`] if hashing fails.
pub async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        let argon2 = configured_argon2()?;
        let password_hash = argon2
            .hash_password(password.as_bytes())
            .map_err(|e| DomainError::InvalidInput(format!("password hashing failed: {e}")))?
            .to_string();
        Ok(password_hash)
    })
    .await
    .map_err(|e| AppError::Internal(format!("hashing task panicked: {e}")))?
}

/// Verify a plaintext password against an Argon2id hash inside `tokio::task::spawn_blocking`.
///
/// # Errors
/// Returns [`AppError`] if hash parsing fails or task panics.
pub async fn verify_password(password_hash: String, password: String) -> Result<bool, AppError> {
    tokio::task::spawn_blocking(move || {
        let parsed_hash = match PasswordHash::new(&password_hash) {
            Ok(h) => h,
            Err(_) => return Ok(false),
        };
        let argon2 = Argon2::default();
        Ok(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    })
    .await
    .map_err(|e| AppError::Internal(format!("verification task panicked: {e}")))?
}
