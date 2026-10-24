//! Authentication, authorization, session management, and CSRF protection.

pub mod argon;
pub mod csrf;
pub mod email;
pub mod extractor;
pub mod limiter;
pub mod password;
pub mod session;

pub use argon::{hash_password, verify_password};
pub use csrf::{CSRF_HEADER_NAME, generate_csrf_token, verify_csrf_token};
pub use email::send_activation_email;
pub use extractor::{AdminUser, AuthUser, Csrf, ManagerUser, OptionalAuthUser, read_cookie};
pub use limiter::RateLimiter;
pub use password::validate_password;
pub use session::{
    SESSION_COOKIE_NAME, build_expired_cookie, build_session_cookie, generate_token, hash_token,
};
