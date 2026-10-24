use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use dp_db::queries::{sessions, users};
use std::sync::Arc;
use tower::ServiceExt;

use dp_web::{AppConfig, AppState, AssetManifest, RateLimiter, create_router, create_template_env};

#[tokio::test]
async fn test_live_auth_end_to_end_flow() {
    let db_url = std::env::var("DP_DATABASE_URL")
        .unwrap_or_else(|_| "mysql://dp:secret@127.0.0.1:3306/dp".to_string());

    // Connect to test database pool
    let pool = match dp_db::create_pool(&db_url, 1, 5).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Skipping live database test: database not reachable at {db_url}");
            return;
        }
    };

    // Ensure migrations are applied
    dp_db::run_migrations(&pool)
        .await
        .expect("migration failed");

    let config = Arc::new(AppConfig::default());
    let manifest = Arc::new(AssetManifest::default());
    let templates = Arc::new(create_template_env(&config, manifest.clone()).unwrap());
    let rate_limiter = Arc::new(RateLimiter::new());

    let state = AppState {
        db: pool.clone(),
        config,
        templates,
        manifest,
        rate_limiter,
    };

    let app = create_router(state);

    let test_user = format!("u_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let test_email = format!("{}@example.org", test_user);
    let test_pass = "SecurePassw0rd123!";

    // 1. POST /register with password too short -> 422 Unprocessable Entity
    let short_form = format!("username={test_user}&email={test_email}&password=short");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(short_form))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // 2. POST /register with valid credentials -> 200 OK with activation message
    let reg_form = format!(
        "username={test_user}&email={test_email}&password={test_pass}&real_name=Nguyen+Van+A"
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(reg_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(res.into_body(), 1024 * 64).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("Đăng ký thành công"));

    // 3. User is pending; attempting login before activation -> 403 Forbidden
    let login_form = format!("username={test_user}&password={test_pass}");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(login_form.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 4. Retrieve activation token directly from database and activate account
    let db_user = users::get_user_by_username(&pool, &test_user)
        .await
        .unwrap()
        .expect("user should exist in database");
    assert_eq!(db_user.status, "pending");
    let token = db_user.activate_token.expect("token must be present");

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/activate/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Check account status is now 'active'
    let activated_user = users::get_user_by_username(&pool, &test_user)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(activated_user.status, "active");
    assert!(activated_user.activate_token.is_none());

    // 5. POST /login with correct password -> 303 See Other with Set-Cookie
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(login_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/");
    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("dp_session="));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));

    // Extract session cookie value
    let cookie_val = set_cookie.split(';').next().unwrap().to_string();

    // 6. GET / with session cookie -> 200 OK and displays logged in username
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .header(header::COOKIE, &cookie_val)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(res.into_body(), 1024 * 64).await.unwrap().to_vec()).unwrap();
    assert!(body.contains(&format!("Chào, <strong>{test_user}</strong>")));
    assert!(body.contains("Đăng xuất"));

    // 7. POST /logout with session cookie -> 303 Redirect to /login and clears cookie
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::COOKIE, &cookie_val)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/login");
    let expired_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(expired_cookie.contains("Max-Age=0"));

    // Verify session was revoked in database
    let raw_token = cookie_val.strip_prefix("dp_session=").unwrap();
    let token_hash = dp_web::auth::session::hash_token(raw_token);
    assert!(
        sessions::lookup_session(&pool, &token_hash)
            .await
            .unwrap()
            .is_none()
    );
}
