//! Integration tests for the Phase 7 diff view (§10).
//!
//! Tests:
//! 1. `GET /projects/{id}/diff` rejects unauthenticated requests.
//! 2. Authenticated users can compare OCR with a round; page text is escaped.
//! 3. `?changes_only=1` hides unchanged lines and `?whitespace=1` marks spaces.
//! 4. Project detail and the proofing toolbar link to the diff view.

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

async fn create_test_user(pool: &MySqlPool, prefix: &str, role: &str) -> (u64, String) {
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

    (uid, raw_token)
}

/// Create a one-page project with a custom OCR baseline and saved P1 text.
async fn setup_diff_project(
    pool: &MySqlPool,
    manager_id: u64,
    ocr_text: &str,
    p1_text: &str,
) -> u64 {
    let rand = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Truyện Kiều diff {rand}");
    let pid = create_project(
        pool,
        &dp_db::queries::projects::NewProject {
            name: &project_name,
            author: "Nguyễn Du",
            tone_style: "either",
            charset: "vi",
            manager_id,
            image_source: "Scans",
            comments: "Ghi chú kiểm thử",
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
            ocr_text: Some(ocr_text.to_string()),
        }],
    )
    .await
    .expect("insert pages failed");

    sqlx::query(
        "UPDATE page_rounds
         SET state = 'done', text = ?, done_at = UTC_TIMESTAMP(6)
         WHERE page_id = (SELECT id FROM pages WHERE project_id = ? AND page_number = 1)
           AND round = 'P1'",
    )
    .bind(p1_text)
    .bind(pid)
    .execute(pool)
    .await
    .expect("save P1 text failed");

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

async fn get_body(app: axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, format!("dp_session={token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn test_diff_requires_authentication() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };
    let app = build_test_app(pool);

    let (status, _) = get_body(app, "/projects/1/diff?page=1", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_diff_renders_escaped_two_column_view() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _) = create_test_user(&pool, "diff_mgr", "pm").await;
    let (_, session_token) = create_test_user(&pool, "diff_proofer", "proofer").await;

    let project_id = setup_diff_project(
        &pool,
        manager_id,
        "<script>alert('ocr')</script>\nDòng chung",
        "Văn bản sạch\nDòng chung",
    )
    .await;
    let app = build_test_app(pool);

    let (status, body) = get_body(
        app,
        &format!("/projects/{project_id}/diff?page=1"),
        Some(&session_token),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("class=\"diff-view\""));
    assert!(body.contains("OCR"));
    assert!(body.contains("Vòng P1"));
    assert!(body.contains("<del"));
    assert!(body.contains("<ins"));
    assert!(body.contains("&lt;script&gt;"));
    assert!(!body.contains("<script>alert('ocr')</script>"));
    assert!(body.contains("Dòng chung"));
    // Project title and both column headers are present.
    assert!(body.contains("So sánh văn bản"));
}

#[tokio::test]
async fn test_diff_changes_only_and_whitespace_toggles() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _) = create_test_user(&pool, "diff_mgr2", "pm").await;
    let (_, session_token) = create_test_user(&pool, "diff_proofer2", "proofer").await;

    let project_id = setup_diff_project(
        &pool,
        manager_id,
        "Dòng giữ nguyên\nmột hai ba",
        "Dòng giữ nguyên\nmột  hai  ba",
    )
    .await;
    let app = build_test_app(pool);

    let full_uri = format!("/projects/{project_id}/diff?page=1");
    let (status, full_body) = get_body(app.clone(), &full_uri, Some(&session_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(full_body.contains("Dòng giữ nguyên"));

    let changes_uri = format!("/projects/{project_id}/diff?page=1&changes_only=1");
    let (status, changes_body) = get_body(app.clone(), &changes_uri, Some(&session_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!changes_body.contains("Dòng giữ nguyên"));
    assert!(changes_body.contains("một"));

    let whitespace_uri = format!("/projects/{project_id}/diff?page=1&whitespace=1");
    let (status, whitespace_body) = get_body(app, &whitespace_uri, Some(&session_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(whitespace_body.contains("class=\"ws\""));
}

#[tokio::test]
async fn test_diff_links_from_project_detail_and_proofing_ui() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (manager_id, _) = create_test_user(&pool, "diff_mgr3", "pm").await;
    let (_, session_token) = create_test_user(&pool, "diff_proofer3", "proofer").await;

    let project_id = setup_diff_project(&pool, manager_id, "OCR text", "P1 text").await;
    let app = build_test_app(pool);

    let (status, detail_body) = get_body(
        app.clone(),
        &format!("/projects/{project_id}"),
        Some(&session_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail_body.contains(&format!("/projects/{project_id}/diff?page=1")));

    // A completed page renders the proofing UI read-only and still exposes the
    // comparison link in its toolbar.
    let (status, proof_body) = get_body(
        app,
        &format!("/projects/{project_id}/pages/1"),
        Some(&session_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(proof_body.contains(&format!("/projects/{project_id}/diff?page=1")));
}
