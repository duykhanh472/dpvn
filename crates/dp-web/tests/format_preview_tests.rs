//! Integration tests for the Phase 9 format-preview route.
//!
//! Tests:
//! 1. Unauthenticated requests are rejected.
//! 2. Missing CSRF is rejected even though the endpoint is read-only.
//! 3. Markup renders to the locked Appendix B shape.
//! 4. Page text is escaped and unclosed markers stay literal.
//! 5. Unknown pages return 404.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use dp_db::queries::{
    pages::{NewPageInput, insert_pages_transactional},
    projects::create_project,
    sessions::create_session,
};
use dp_web::{AppConfig, AppState, AssetManifest, RateLimiter, create_router, create_template_env};
use sqlx::MySqlPool;
use std::sync::Arc;
use tower::ServiceExt;

async fn get_test_pool() -> Option<MySqlPool> {
    let db_url = std::env::var("DP_DATABASE_URL")
        .unwrap_or_else(|_| "mysql://dp:secret@127.0.0.1:3306/dp".to_string());

    let pool = dp_db::create_pool(&db_url, 1, 10).await.ok()?;
    dp_db::run_migrations(&pool).await.ok()?;
    Some(pool)
}

async fn create_test_user(pool: &MySqlPool, prefix: &str, role: &str) -> (u64, String, String) {
    let rand_id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let username = format!("{prefix}_{rand_id}");
    let email = format!("{username}@example.com");
    let hash = "$argon2id$v=19$m=19456,t=2,p=1$fake$hash";

    let res = sqlx::query(
        "INSERT INTO users (username, email, password_hash, real_name, status, activate_token, created_at)
         VALUES (?, ?, ?, '', 'active', NULL, UTC_TIMESTAMP(6))",
    )
    .bind(&username)
    .bind(&email)
    .bind(hash)
    .execute(pool)
    .await
    .expect("create user failed");

    let uid = res.last_insert_id();

    sqlx::query(
        "INSERT INTO user_roles (user_id, role, granted_at) VALUES (?, ?, UTC_TIMESTAMP(6))",
    )
    .bind(uid)
    .bind(role)
    .execute(pool)
    .await
    .expect("grant role failed");

    let raw_token = dp_web::auth::generate_token();
    let token_hash = dp_web::auth::hash_token(&raw_token);
    let csrf_token = dp_web::auth::generate_token();

    create_session(pool, &token_hash, uid, &csrf_token, None, "test-agent")
        .await
        .expect("create session failed");

    (uid, raw_token, csrf_token)
}

async fn setup_project(pool: &MySqlPool, manager_id: u64) -> u64 {
    let rand = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Phase9 Preview {rand}");

    let pid = create_project(
        pool,
        &dp_db::queries::projects::NewProject {
            name: &project_name,
            author: "Nguyễn Du",
            tone_style: "either",
            charset: "vi",
            manager_id,
            image_source: "Scans",
            comments: "",
        },
    )
    .await
    .expect("create project failed");

    insert_pages_transactional(
        pool,
        pid,
        &[NewPageInput {
            page_number: 1,
            extension: "png".to_string(),
            image_width: 1500,
            image_height: 2200,
            ocr_text: Some("Nội dung OCR".to_string()),
        }],
    )
    .await
    .expect("insert pages failed");

    sqlx::query(
        "UPDATE projects
         SET phase = 'in_round', current_round = 'P1', updated_at = UTC_TIMESTAMP(6)
         WHERE id = ?",
    )
    .bind(pid)
    .execute(pool)
    .await
    .expect("move project to in_round failed");

    pid
}

fn build_test_app(pool: MySqlPool) -> axum::Router {
    let config = Arc::new(AppConfig::default());
    let manifest = Arc::new(AssetManifest::default());
    let templates = Arc::new(create_template_env(&config, manifest.clone()).unwrap());
    let rate_limiter = Arc::new(RateLimiter::new());

    let state = AppState {
        db: pool,
        config,
        templates,
        manifest,
        rate_limiter,
        site_words: Arc::new(tokio::sync::RwLock::new(Default::default())),
    };

    create_router(state)
}

fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode_component(key), encode_component(value)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn post_preview(
    app: axum::Router,
    project_id: u64,
    page_number: i32,
    token: Option<&str>,
    csrf: Option<&str>,
    text: &str,
) -> (StatusCode, String) {
    let body = form_body(&[("text", text)]);
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/projects/{project_id}/pages/{page_number}/preview"
        ))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, format!("dp_session={token}"));
    }
    if let Some(csrf) = csrf {
        builder = builder.header("X-CSRF-Token", csrf);
    }

    let response = app
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn get_page(app: axum::Router, uri: &str, token: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(uri)
        .header(header::COOKIE, format!("dp_session={token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn test_preview_requires_authentication() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };
    let app = build_test_app(pool);

    let (status, _) = post_preview(app, 1, 1, None, Some("token"), "text").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_preview_rejects_missing_csrf() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p9_cmgr", "pm").await;
    let (_, token, _) = create_test_user(&pool, "p9_cuser", "proofer").await;
    let project_id = setup_project(&pool, manager_id).await;
    let app = build_test_app(pool);

    let (status, _) = post_preview(app, project_id, 1, Some(&token), None, "text").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_preview_renders_locked_markup() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p9_rmgr", "pm").await;
    let (_, token, csrf) = create_test_user(&pool, "p9_ruser", "proofer").await;
    let project_id = setup_project(&pool, manager_id).await;
    let app = build_test_app(pool);

    let source = "Đoạn _nghiêng_ và *đậm*.\n\n# Trích dẫn\n\n/>  Thơ\n\n----\n\n\
                  <sc>Việt Nam</sc>\n\n[Footnote 1: Ghi chú]";
    let (status, body) = post_preview(app, project_id, 1, Some(&token), Some(&csrf), source).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("class=\"format-preview\""), "body: {body}");
    assert!(body.contains("<i>nghiêng</i>"), "body: {body}");
    assert!(body.contains("<b>đậm</b>"), "body: {body}");
    assert!(
        body.contains("<blockquote><p>Trích dẫn</p></blockquote>"),
        "body: {body}"
    );
    assert!(
        body.contains("<div class=\"poem-line\">  Thơ</div>"),
        "body: {body}"
    );
    assert!(body.contains("<hr>"), "body: {body}");
    assert!(
        body.contains("<span class=\"sc\">VIỆT NAM</span>"),
        "body: {body}"
    );
    assert!(body.contains("<ol class=\"footnotes\">"), "body: {body}");
}

#[tokio::test]
async fn test_preview_escapes_text_and_keeps_unclosed_markers_literal() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p9_emgr", "pm").await;
    let (_, token, csrf) = create_test_user(&pool, "p9_euser", "proofer").await;
    let project_id = setup_project(&pool, manager_id).await;
    let app = build_test_app(pool);

    let source = "_chưa đóng <script>alert('x')</script>";
    let (status, body) = post_preview(app, project_id, 1, Some(&token), Some(&csrf), source).await;

    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("<script"), "body: {body}");
    assert!(body.contains("&lt;script&gt;"), "body: {body}");
    assert!(body.contains("_chưa đóng"), "body: {body}");
    assert!(!body.contains("<i>"), "body: {body}");
}

#[tokio::test]
async fn test_preview_unknown_page_returns_404() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p9_nmgr", "pm").await;
    let (_, token, csrf) = create_test_user(&pool, "p9_nuser", "proofer").await;
    let project_id = setup_project(&pool, manager_id).await;
    let app = build_test_app(pool);

    let (status, _) = post_preview(app, project_id, 99, Some(&token), Some(&csrf), "text").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_proofing_ui_exposes_the_preview_pane() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p9_umgr", "pm").await;
    let (_, token, _) = create_test_user(&pool, "p9_uuser", "proofer").await;
    let project_id = setup_project(&pool, manager_id).await;
    let app = build_test_app(pool);

    let (status, body) = get_page(app, &format!("/projects/{project_id}/pages/1"), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("id=\"btn-toggle-preview\""), "body: {body}");
    assert!(body.contains("id=\"format-preview\""), "body: {body}");
    assert!(
        body.contains(&format!(
            "hx-post=\"/projects/{project_id}/pages/1/preview\""
        )),
        "body: {body}"
    );
    assert!(body.contains("id=\"format-preview-body\""), "body: {body}");
}
