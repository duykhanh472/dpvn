//! Password policy validation and common password checking.

use dp_core::DomainError;

/// List of common vulnerable passwords that are rejected outright.
const COMMON_PASSWORDS: &[&str] = &[
    "1234567890",
    "password123",
    "qwertyuiop",
    "admin12345",
    "iloveyou12",
    "dproofreader",
    "proofreading",
    "12345678901",
    "welcome1234",
    "passphrase1",
];

/// Validate that a password meets minimum complexity and length requirements (§12.1).
///
/// # Errors
/// Returns [`DomainError::InvalidInput`] if the password is too short, too long, or common.
pub fn validate_password(password: &str) -> Result<(), DomainError> {
    let char_count = password.chars().count();

    if char_count < 10 {
        return Err(DomainError::InvalidInput(
            "mật khẩu phải có ít nhất 10 ký tự".to_string(),
        ));
    }

    if char_count > 128 {
        return Err(DomainError::InvalidInput(
            "mật khẩu không được vượt quá 128 ký tự".to_string(),
        ));
    }

    let lower = password.to_lowercase();
    for &common in COMMON_PASSWORDS {
        if lower == common {
            return Err(DomainError::InvalidInput(
                "mật khẩu quá phổ biến, vui lòng chọn mật khẩu an toàn hơn".to_string(),
            ));
        }
    }

    Ok(())
}
