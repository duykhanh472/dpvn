//! Axum extractors for authenticated users, role-based access control, and CSRF protection.

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, Method, header, request::Parts},
};
use dp_core::UserId;
use dp_db::queries::{sessions, users};

use crate::app::{AppError, AppState};
use crate::auth::csrf::{CSRF_HEADER_NAME, verify_csrf_token};
use crate::auth::session::{SESSION_COOKIE_NAME, hash_token};

/// Authenticated user extracted from a valid session cookie.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// User ID.
    pub user_id: UserId,
    /// Username.
    pub username: String,
    /// Verified email address.
    pub email: String,
    /// Real / display name.
    pub real_name: String,
    /// Session-bound CSRF token.
    pub csrf_token: String,
    /// Granted user roles (e.g. 'proofer', 'pm', 'pp', 'admin').
    pub roles: Vec<String>,
}

impl AuthUser {
    /// Check if the user has been granted a specific role.
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check if the user has administrative privileges.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.has_role("admin")
    }

    /// Check if the user has project manager (or admin) privileges.
    #[must_use]
    pub fn is_pm(&self) -> bool {
        self.has_role("pm") || self.is_admin()
    }

    /// Check if the user has post-processor (or admin) privileges.
    #[must_use]
    pub fn is_pp(&self) -> bool {
        self.has_role("pp") || self.is_admin()
    }
}

/// Helper function to parse a cookie by name from incoming HTTP headers.
pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for cookie_hdr in headers.get_all(header::COOKIE) {
        if let Ok(s) = cookie_hdr.to_str() {
            for pair in s.split(';') {
                let mut parts = pair.trim().splitn(2, '=');
                if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                    if k.trim() == name {
                        return Some(v.trim().to_string());
                    }
                }
            }
        }
    }
    None
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let cookie =
            read_cookie(&parts.headers, SESSION_COOKIE_NAME).ok_or(AppError::Unauthorized)?;

        let token_hash = hash_token(&cookie);
        let session_user = sessions::lookup_session(&state.db, &token_hash)
            .await?
            .ok_or(AppError::Unauthorized)?;

        let roles = users::get_user_roles(&state.db, session_user.user_id).await?;

        // Background touch: update last_seen_at if older than 5 minutes
        let db_clone = state.db.clone();
        tokio::spawn(async move {
            let _ = sessions::touch_session(&db_clone, &token_hash).await;
        });

        Ok(Self {
            user_id: UserId::new(session_user.user_id),
            username: session_user.username,
            email: session_user.email,
            real_name: session_user.real_name,
            csrf_token: session_user.csrf_token,
            roles,
        })
    }
}

/// Optional authentication extractor for public endpoints that display personalized headers.
#[derive(Debug, Clone)]
pub struct OptionalAuthUser(pub Option<AuthUser>);

impl FromRequestParts<AppState> for OptionalAuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match AuthUser::from_request_parts(parts, state).await {
            Ok(user) => Ok(Self(Some(user))),
            Err(AppError::Unauthorized) => Ok(Self(None)),
            Err(e) => Err(e),
        }
    }
}

/// Extractor that requires admin role, rejecting unauthorized requests with 403 Forbidden.
#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;
        if !auth.is_admin() {
            return Err(AppError::Forbidden);
        }
        Ok(Self(auth))
    }
}

/// Extractor that requires project manager or admin role.
#[derive(Debug, Clone)]
pub struct ManagerUser(pub AuthUser);

impl FromRequestParts<AppState> for ManagerUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;
        if !auth.is_pm() {
            return Err(AppError::Forbidden);
        }
        Ok(Self(auth))
    }
}

/// Extractor ensuring CSRF token validity on mutating HTTP requests.
#[derive(Debug, Clone)]
pub struct Csrf;

impl FromRequestParts<AppState> for Csrf {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Safe read-only HTTP methods do not require CSRF token
        if parts.method == Method::GET
            || parts.method == Method::HEAD
            || parts.method == Method::OPTIONS
        {
            return Ok(Self);
        }

        let auth = AuthUser::from_request_parts(parts, state).await?;

        let submitted = parts
            .headers
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok());

        let Some(submitted_token) = submitted else {
            return Err(AppError::Forbidden);
        };

        if !verify_csrf_token(submitted_token, &auth.csrf_token) {
            return Err(AppError::Forbidden);
        }

        Ok(Self)
    }
}
