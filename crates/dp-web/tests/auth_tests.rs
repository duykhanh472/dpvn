use dp_web::auth::{
    argon::{hash_password, verify_password},
    csrf::{generate_csrf_token, verify_csrf_token},
    limiter::RateLimiter,
    password::validate_password,
    session::{
        SESSION_COOKIE_NAME, build_expired_cookie, build_session_cookie, generate_token, hash_token,
    },
};
use std::net::IpAddr;

#[tokio::test]
async fn test_argon2id_hash_and_verify() {
    let password = "SuperSecretPassword123!".to_string();
    let hash = hash_password(password.clone()).await.unwrap();

    assert!(hash.starts_with("$argon2id$"));
    assert!(verify_password(hash.clone(), password).await.unwrap());
    assert!(
        !verify_password(hash, "WrongPassword!".to_string())
            .await
            .unwrap()
    );
}

#[test]
fn test_password_policy_validation() {
    // Valid password
    assert!(validate_password("ValidP@ssw0rd1").is_ok());

    // Too short (< 10 chars)
    let err = validate_password("Short1!").unwrap_err();
    assert!(err.to_string().contains("ít nhất 10 ký tự"));

    // Common password blacklist
    let err = validate_password("password123").unwrap_err();
    assert!(err.to_string().contains("quá phổ biến"));

    let err = validate_password("1234567890").unwrap_err();
    assert!(err.to_string().contains("quá phổ biến"));
}

#[test]
fn test_session_token_and_cookie() {
    let token = generate_token();
    assert_eq!(token.len(), 43);

    let hash1 = hash_token(&token);
    let hash2 = hash_token(&token);
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 32);

    let secure_cookie = build_session_cookie(&token, true);
    assert!(secure_cookie.contains("HttpOnly"));
    assert!(secure_cookie.contains("Secure"));
    assert!(secure_cookie.contains("SameSite=Lax"));
    assert!(secure_cookie.contains(SESSION_COOKIE_NAME));

    let dev_cookie = build_session_cookie(&token, false);
    assert!(!dev_cookie.contains("Secure"));

    let expired = build_expired_cookie();
    assert!(expired.contains("Max-Age=0"));
}

#[test]
fn test_csrf_token_verification() {
    let token = generate_csrf_token();
    assert_eq!(token.len(), 43);

    assert!(verify_csrf_token(&token, &token));
    assert!(!verify_csrf_token(&token, "invalid_csrf_token"));
    assert!(!verify_csrf_token(&token, ""));
}

#[test]
fn test_rate_limiter() {
    let limiter = RateLimiter::new();
    let ip: IpAddr = "192.168.1.100".parse().unwrap();

    // Registration allows 5 attempts
    for _ in 0..5 {
        assert!(limiter.check_registration(ip));
    }
    // 6th attempt is blocked
    assert!(!limiter.check_registration(ip));

    // Login allows 5 attempts for same username
    for _ in 0..5 {
        assert!(limiter.check_login(ip, "user1"));
    }
    assert!(!limiter.check_login(ip, "user1"));

    // Different user still has attempts from same IP (until 10 max IP attempts)
    assert!(limiter.check_login(ip, "user2"));
}
