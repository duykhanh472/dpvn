//! Authentication route handlers: registration, email activation, login, and logout.

use axum::{
    Form,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use dp_core::DomainError;
use dp_db::queries::{sessions, users};
use minijinja::context;
use regex::Regex;
use serde::Deserialize;
use std::net::IpAddr;

use crate::app::{AppError, AppState};
use crate::auth::{
    AuthUser, build_expired_cookie, build_session_cookie, generate_csrf_token, generate_token,
    hash_password, hash_token, send_activation_email, validate_password, verify_password,
};
use crate::settings::SiteSettings;

/// Form submission data for user registration.
#[derive(Debug, Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub real_name: String,
}

/// Form submission data for user login.
#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

/// Helper to determine the client IP from standard proxy headers or defaults.
fn get_client_ip(headers: &HeaderMap) -> IpAddr {
    if let Some(forwarded) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first_ip) = forwarded.split(',').next() {
            if let Ok(ip) = first_ip.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if let Ok(ip) = real_ip.trim().parse::<IpAddr>() {
            return ip;
        }
    }
    "127.0.0.1".parse().unwrap()
}

/// Validate username format and length.
fn validate_username(username: &str) -> Result<(), DomainError> {
    let len = username.chars().count();
    if !(3..=32).contains(&len) {
        return Err(DomainError::InvalidInput(
            "tên người dùng phải từ 3 đến 32 ký tự".to_string(),
        ));
    }

    let re = Regex::new(r"^[a-zA-Z0-9_.-]+$").unwrap();
    if !re.is_match(username) {
        return Err(DomainError::InvalidInput(
            "tên người dùng chỉ được chứa chữ cái, số, dấu gạch dưới, gạch ngang và dấu chấm"
                .to_string(),
        ));
    }

    Ok(())
}

/// Validate email address format.
fn validate_email(email: &str) -> Result<(), DomainError> {
    let len = email.chars().count();
    if !(5..=255).contains(&len) || !email.contains('@') || !email.contains('.') {
        return Err(DomainError::InvalidInput(
            "địa chỉ email không hợp lệ".to_string(),
        ));
    }
    Ok(())
}

/// Render the registration form.
pub async fn register_page(State(state): State<AppState>) -> Result<Response, AppError> {
    let site_settings = SiteSettings::load(&state.db, &state.config.limits).await?;
    let tmpl = state.templates.get_template("auth/register.html")?;
    let rendered = tmpl.render(context! {
        csrf_token => "",
        error => None::<String>,
        success => None::<String>,
        registration_open => site_settings.registration_open,
    })?;
    Ok(Html(rendered).into_response())
}

/// Process registration form submission.
pub async fn register_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<Response, AppError> {
    let site_settings = SiteSettings::load(&state.db, &state.config.limits).await?;
    if !site_settings.registration_open {
        let tmpl = state.templates.get_template("auth/register.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some("Đăng ký tài khoản đang tạm đóng. Vui lòng liên hệ quản trị viên.".to_string()),
            success => None::<String>,
            registration_open => false,
        })?;
        return Ok((StatusCode::FORBIDDEN, Html(rendered)).into_response());
    }

    let client_ip = get_client_ip(&headers);

    // 1. Rate limiting: 5 registrations per hour per IP
    if !state.rate_limiter.check_registration(client_ip) {
        return Err(AppError::Domain(DomainError::DailyLimitReached {
            limit: 5,
        }));
    }

    // 2. Validate input fields
    let username = form.username.trim();
    let email = form.email.trim();
    let real_name = form.real_name.trim();

    if let Err(err) = validate_username(username) {
        let tmpl = state.templates.get_template("auth/register.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some(err.to_string()),
            success => None::<String>,
            username => username,
            email => email,
            real_name => real_name,
        })?;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(rendered)).into_response());
    }

    if let Err(err) = validate_email(email) {
        let tmpl = state.templates.get_template("auth/register.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some(err.to_string()),
            success => None::<String>,
            username => username,
            email => email,
            real_name => real_name,
        })?;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(rendered)).into_response());
    }

    if let Err(err) = validate_password(&form.password) {
        let tmpl = state.templates.get_template("auth/register.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some(err.to_string()),
            success => None::<String>,
            username => username,
            email => email,
            real_name => real_name,
        })?;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(rendered)).into_response());
    }

    // 3. Check for existing username or email
    if users::get_user_by_username(&state.db, username)
        .await?
        .is_some()
    {
        let tmpl = state.templates.get_template("auth/register.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some("tên người dùng này đã được sử dụng".to_string()),
            success => None::<String>,
            username => username,
            email => email,
            real_name => real_name,
        })?;
        return Ok((StatusCode::CONFLICT, Html(rendered)).into_response());
    }

    if users::get_user_by_email(&state.db, email).await?.is_some() {
        let tmpl = state.templates.get_template("auth/register.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some("địa chỉ email này đã được đăng ký".to_string()),
            success => None::<String>,
            username => username,
            email => email,
            real_name => real_name,
        })?;
        return Ok((StatusCode::CONFLICT, Html(rendered)).into_response());
    }

    // 4. Hash password with Argon2id
    let password_hash = hash_password(form.password).await?;
    let activate_token = generate_token();

    // 5. Store user in database with 'pending' status
    users::create_user(
        &state.db,
        username,
        email,
        &password_hash,
        real_name,
        &activate_token,
    )
    .await?;

    // 6. Send activation email
    send_activation_email(
        &state.config.mail.smtp_url,
        &state.config.mail.from,
        &state.config.server.public_url,
        email,
        username,
        &activate_token,
    )
    .await?;

    let tmpl = state.templates.get_template("auth/register.html")?;
    let rendered = tmpl.render(context! {
        csrf_token => "",
        error => None::<String>,
        success => Some("Đăng ký thành công! Vui lòng kiểm tra hộp thư email của bạn để kích hoạt tài khoản.".to_string()),
    })?;

    Ok(Html(rendered).into_response())
}

/// Activate account using token sent via email.
pub async fn activate_account(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let success = users::activate_user(&state.db, &token).await?;

    let tmpl = state.templates.get_template("auth/activate.html")?;
    let rendered = tmpl.render(context! {
        csrf_token => "",
        success => success,
    })?;

    Ok(Html(rendered).into_response())
}

/// Render the login form.
pub async fn login_page(State(state): State<AppState>) -> Result<Response, AppError> {
    let tmpl = state.templates.get_template("auth/login.html")?;
    let rendered = tmpl.render(context! {
        csrf_token => "",
        error => None::<String>,
    })?;
    Ok(Html(rendered).into_response())
}

/// Process login form submission.
pub async fn login_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    let client_ip = get_client_ip(&headers);
    let username = form.username.trim();

    // 1. Rate limiting: 10 per 15 min per IP, 5 per username
    if !state.rate_limiter.check_login(client_ip, username) {
        let tmpl = state.templates.get_template("auth/login.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some("Quá nhiều lần đăng nhập không thành công. Vui lòng thử lại sau 15 phút.".to_string()),
            username => username,
        })?;
        return Ok((StatusCode::TOO_MANY_REQUESTS, Html(rendered)).into_response());
    }

    // 2. Fetch user by username
    let user_opt = users::get_user_by_username(&state.db, username).await?;

    let Some(user) = user_opt else {
        let tmpl = state.templates.get_template("auth/login.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some("Tên đăng nhập hoặc mật khẩu không chính xác.".to_string()),
            username => username,
        })?;
        return Ok((StatusCode::UNAUTHORIZED, Html(rendered)).into_response());
    };

    // 3. Verify user status
    if user.status != "active" {
        let tmpl = state.templates.get_template("auth/login.html")?;
        let msg = if user.status == "pending" {
            "Tài khoản chưa được kích hoạt. Vui lòng kiểm tra email của bạn để kích hoạt."
        } else {
            "Tài khoản của bạn đã bị vô hiệu hóa."
        };
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some(msg.to_string()),
            username => username,
        })?;
        return Ok((StatusCode::FORBIDDEN, Html(rendered)).into_response());
    }

    // 4. Verify password with Argon2id
    let valid = verify_password(user.password_hash.clone(), form.password).await?;
    if !valid {
        let tmpl = state.templates.get_template("auth/login.html")?;
        let rendered = tmpl.render(context! {
            csrf_token => "",
            error => Some("Tên đăng nhập hoặc mật khẩu không chính xác.".to_string()),
            username => username,
        })?;
        return Ok((StatusCode::UNAUTHORIZED, Html(rendered)).into_response());
    }

    // 5. Update last login timestamp
    users::update_last_login(&state.db, user.id).await?;

    // 6. Issue session token and CSRF token
    let session_token = generate_token();
    let token_hash = hash_token(&session_token);
    let csrf_token = generate_csrf_token();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    sessions::create_session(
        &state.db,
        &token_hash,
        user.id,
        &csrf_token,
        None,
        user_agent,
    )
    .await?;

    let is_secure = state.config.server.public_url.starts_with("https://");
    let cookie_header = build_session_cookie(&session_token, is_secure);

    // 7. Redirect 303 to home dashboard with session cookie
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/"),
            (header::SET_COOKIE, &cookie_header),
        ],
    )
        .into_response())
}

/// Logout handler: invalidates session in database and expires session cookie.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    _auth: AuthUser,
) -> Result<Response, AppError> {
    if let Some(cookie) = crate::auth::read_cookie(&headers, crate::auth::SESSION_COOKIE_NAME) {
        let token_hash = hash_token(&cookie);
        let _ = sessions::delete_session(&state.db, &token_hash).await;
    }

    let expired_cookie = build_expired_cookie();
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/login"),
            (header::SET_COOKIE, &expired_cookie),
        ],
    )
        .into_response())
}
