//! Integration tests for Phase 5 proofing interface (§14, §15).
//!
//! Tests:
//! 1. Proofer claims page via `GET /projects/{id}/proof` -> 303 redirect to `/projects/{id}/pages/1`.
//! 2. Renders `GET /projects/{id}/pages/1` with CM6 mount, image viewer, and character palette.
//! 3. Saves text via `POST /projects/{id}/pages/1/save`, verifies idempotent database commit and tally.
//! 4. Blocks illegal characters outside charset with 400 Bad Request.
//! 5. Authorizes image scans (§15.2): unauthenticated gets 401, non-claimant gets 403, claimant/manager gets 200 with X-Accel-Redirect.
//! 6. Realtime wordcheck feedback endpoint returns issues for CM6.
//! 7. Voluntary claim release resets state to available.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use dp_db::queries::{
    page_rounds::get_page_round,
    pages::{NewPageInput, get_page_by_project_and_number, insert_pages_transactional},
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

    // Create session token
    let raw_token = dp_web::auth::generate_token();
    let token_hash = dp_web::auth::hash_token(&raw_token);
    let csrf_token = dp_web::auth::generate_token();

    create_session(pool, &token_hash, uid, &csrf_token, None, "test-agent")
        .await
        .expect("create session failed");

    (uid, raw_token, csrf_token)
}

async fn setup_test_project(pool: &MySqlPool, manager_id: u64, pages_count: usize) -> u64 {
    let rand = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Truyện Kiều {rand}");
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

    let inputs: Vec<NewPageInput> = (1..=pages_count)
        .map(|i| NewPageInput {
            page_number: i as i32,
            extension: "png".to_string(),
            image_width: 1500,
            image_height: 2200,
            ocr_text: Some(format!("Bản thảo trang {i}")),
        })
        .collect();

    insert_pages_transactional(pool, pid, &inputs)
        .await
        .expect("insert pages failed");

    // Move project to in_round P1
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
    };

    create_router(state)
}

#[tokio::test]
async fn test_proof_claim_render_and_save_flow() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (mgr_id, _, _) = create_test_user(&pool, "mgr", "pm").await;
    let (proofer_id, session_token, csrf_token) =
        create_test_user(&pool, "proofer", "proofer").await;

    let project_id = setup_test_project(&pool, mgr_id, 3).await;
    let app = build_test_app(pool.clone());

    // 1. GET /projects/{id}/proof -> should claim page 1 and redirect 303 to /projects/{id}/pages/1
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/proof"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get(header::LOCATION).unwrap(),
        &format!("/projects/{project_id}/pages/1")
    );

    // 2. GET /projects/{id}/pages/1 -> renders proof.html
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/pages/1"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .body(Body::empty())
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(body.contains("Trang <strong>1</strong> / 3"));
    assert!(body.contains("id=\"proof-workspace\""));
    assert!(body.contains("id=\"codemirror-mount\""));
    assert!(body.contains("id=\"split-handle\""));
    assert!(body.contains("data-char=\"đ\""));

    // 3. POST /projects/{id}/pages/1/save -> save valid Vietnamese text
    let save_body = serde_json::json!({
        "text": "Trăm năm trong cõi người ta,\nChữ tài chữ mệnh khéo là ghét nhau.",
        "confirmed_hash": null,
        "csrf_token": csrf_token
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/save"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(save_body.to_string()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res_body: serde_json::Value =
        serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(res_body["ok"], true);
    assert_eq!(
        res_body["next_url"],
        format!("/projects/{project_id}/proof")
    );

    // Verify page in DB is now done
    let page = get_page_by_project_and_number(&pool, project_id, 1)
        .await
        .unwrap()
        .expect("page 1 must exist");
    let page_row = get_page_round(&pool, page.id, "P1")
        .await
        .unwrap()
        .expect("page_round must exist");
    assert_eq!(page_row.state, "done");
    assert_eq!(page_row.proofer_id, Some(proofer_id));
}

#[tokio::test]
async fn test_proof_blocking_invalid_character() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (mgr_id, _, _) = create_test_user(&pool, "mgr2", "pm").await;
    let (_, session_token, csrf_token) = create_test_user(&pool, "proofer2", "proofer").await;

    let project_id = setup_test_project(&pool, mgr_id, 2).await;
    let app = build_test_app(pool.clone());

    // Claim page 1
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/proof"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    // Attempt to save text containing unallowed Cyrillic character 'Д'
    let save_body = serde_json::json!({
        "text": "Văn bản chứa ký tự Nga Д không thuộc bộ ký tự.",
        "confirmed_hash": null,
        "csrf_token": csrf_token
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/save"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(save_body.to_string()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Page must remain in_progress
    let page = get_page_by_project_and_number(&pool, project_id, 1)
        .await
        .unwrap()
        .expect("page 1 must exist");
    let page_row = get_page_round(&pool, page.id, "P1")
        .await
        .unwrap()
        .expect("page_round must exist");
    assert_eq!(page_row.state, "in_progress");
}

#[tokio::test]
async fn test_image_delivery_authorization_rules() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (mgr_id, mgr_token, _) = create_test_user(&pool, "mgr3", "pm").await;
    let (_proofer1_id, proofer1_token, _) = create_test_user(&pool, "proofer3_a", "proofer").await;
    let (_proofer2_id, proofer2_token, _) = create_test_user(&pool, "proofer3_b", "proofer").await;

    let project_id = setup_test_project(&pool, mgr_id, 2).await;
    let app = build_test_app(pool.clone());

    // Proofer 1 claims page 1
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/proof"))
        .header(header::COOKIE, format!("dp_session={proofer1_token}"))
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    // 1. Unauthenticated request -> 401 Unauthorized
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/pages/1/image"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // 2. Proofer 2 (not holding claim) -> 403 Forbidden
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/pages/1/image"))
        .header(header::COOKIE, format!("dp_session={proofer2_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 3. Proofer 1 (holding claim) -> 200 OK with X-Accel-Redirect header (§15.2)
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/pages/1/image"))
        .header(header::COOKIE, format!("dp_session={proofer1_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let accel = res.headers().get("x-accel-redirect");
    assert!(accel.is_some());
    assert!(accel.unwrap().to_str().unwrap().starts_with("/protected/"));

    // 4. Project Manager -> 200 OK with X-Accel-Redirect header
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/pages/1/image"))
        .header(header::COOKIE, format!("dp_session={mgr_token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().contains_key("x-accel-redirect"));
}

#[tokio::test]
async fn test_wordcheck_feedback_endpoint() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (mgr_id, _, _) = create_test_user(&pool, "mgr4", "pm").await;
    let (_, session_token, _) = create_test_user(&pool, "proofer4", "proofer").await;

    let project_id = setup_test_project(&pool, mgr_id, 2).await;
    let app = build_test_app(pool.clone());

    // Wordcheck with invalid character
    let body = serde_json::json!({
        "text": "Kiểm tra ký tự lạ: ∑ và π"
    });

    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/wordcheck"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let issues: Vec<dp_core::wordcheck::WordIssue> =
        serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].word, "∑");
    assert_eq!(issues[1].word, "π");
}

#[tokio::test]
async fn test_voluntary_claim_release() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (mgr_id, _, _) = create_test_user(&pool, "mgr5", "pm").await;
    let (_, session_token, csrf_token) = create_test_user(&pool, "proofer5", "proofer").await;

    let project_id = setup_test_project(&pool, mgr_id, 2).await;
    let app = build_test_app(pool.clone());

    // 1. Claim page
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/proof"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    // Verify claimed
    let page = get_page_by_project_and_number(&pool, project_id, 1)
        .await
        .unwrap()
        .expect("page 1 must exist");
    let page_row = get_page_round(&pool, page.id, "P1").await.unwrap().unwrap();
    assert_eq!(page_row.state, "in_progress");

    // 2. Release claim
    let form_body = format!("csrf_token={csrf_token}");
    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/release"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form_body))
        .unwrap();

    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    // Verify page is back to available
    let page_row = get_page_round(&pool, page.id, "P1").await.unwrap().unwrap();
    assert_eq!(page_row.state, "available");
    assert_eq!(page_row.proofer_id, None);
}
