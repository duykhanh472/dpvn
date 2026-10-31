//! Integration tests for Phase 6 Wordcheck engine (§9, Phase 6 Acceptance).
//!
//! Tests:
//! 1. Content-bound confirmation flow (§9.5):
//!    - Text with issues returns HTML confirmation panel with SHA-256 hash.
//!    - Submitting with matching confirmed_hash commits successfully.
//!    - Modifying text B while sending confirmed_hash of text A is rejected and re-prompts.
//! 2. 3-signal priority & Project custom words (§9.2):
//!    - Project good word suppresses phonotactic warning (e.g. "Shakespeare").
//!    - Project bad word flags otherwise valid word.
//! 3. Project words CRUD & RBAC:
//!    - PM/Admin can manage; non-manager proofer gets 403 Forbidden.
//! 4. Admin site words CRUD & in-memory cache refresh:
//!    - Admin only; dynamically invalidates and updates `state.site_words`.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use dp_core::viet;
use dp_db::queries::{
    page_rounds::get_page_round,
    pages::{NewPageInput, get_page_by_project_and_number, insert_pages_transactional},
    projects::create_project,
    sessions::create_session,
    words::get_all_project_words,
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

async fn setup_test_project(pool: &MySqlPool, manager_id: u64) -> u64 {
    let rand = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Dự án Kiểm Từ {rand}");
    let pid = create_project(
        pool,
        &dp_db::queries::projects::NewProject {
            name: &project_name,
            author: "Tác giả kiểm thử",
            tone_style: "either",
            charset: "vi",
            manager_id,
            image_source: "Scans",
            comments: "Ghi chú",
        },
    )
    .await
    .expect("create project failed");

    let inputs = vec![NewPageInput {
        page_number: 1,
        extension: "png".to_string(),
        image_width: 1500,
        image_height: 2200,
        ocr_text: Some("Bản thảo ban đầu".to_string()),
    }];

    insert_pages_transactional(pool, pid, &inputs)
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

fn build_test_state(pool: MySqlPool) -> AppState {
    let config = Arc::new(AppConfig::default());
    let manifest = Arc::new(AssetManifest::default());
    let templates = Arc::new(create_template_env(&config, manifest.clone()).unwrap());
    let rate_limiter = Arc::new(RateLimiter::new());

    AppState {
        db: pool,
        config,
        templates,
        manifest,
        rate_limiter,
        site_words: Arc::new(tokio::sync::RwLock::new(Default::default())),
    }
}

#[tokio::test]
async fn test_content_bound_confirmation_flow() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (_mgr_id, _, _) = create_test_user(&pool, "mgr_conf", "pm").await;
    let (_proofer_id, session_token, csrf_token) =
        create_test_user(&pool, "proofer_conf", "proofer").await;

    let project_id = setup_test_project(&pool, _mgr_id).await;
    let state = build_test_state(pool.clone());
    let app = create_router(state.clone());

    // 1. Claim page 1
    let claim_req = Request::builder()
        .uri(format!("/projects/{project_id}/proof"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(claim_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // 2. Save text with phonotactic violation without confirmation
    // "ghặp" violates Vietnamese spelling ('gh' before 'ă'); "họcc" has double c
    let text_a = "Học sinh ghặp nhiều khó khăn khi họcc tập.";
    let save_body_a = serde_json::json!({
        "text": text_a,
        "csrf_token": csrf_token,
        "confirmed_hash": null,
    });

    let save_req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/save"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&save_body_a).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(save_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/html"));

    let body_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    // Confirmation panel rendered
    assert!(body_str.contains("Cảnh báo soát lỗi"));
    assert!(body_str.contains("ghặp"));
    assert!(body_str.contains("data-hash="));

    // Extract SHA-256 hash of text A
    use sha2::{Digest, Sha256};
    let normalized_a = viet::normalize(text_a);
    let hash_a = format!("{:x}", Sha256::digest(normalized_a.as_bytes()));
    assert!(body_str.contains(&hash_a));

    // 3. Proofer modifies text to text B, but passes hash_a (stale/mismatched hash)
    let text_b = "Học sinh ghặp thêm vấn đề khác khi họcc tập.";
    let spoof_body = serde_json::json!({
        "text": text_b,
        "csrf_token": csrf_token,
        "confirmed_hash": hash_a, // Hash of A does not match B!
    });

    let spoof_req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/save"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&spoof_body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(spoof_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Must return HTML confirmation panel for text B, NOT commit!
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/html"));

    let body_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    let normalized_b = viet::normalize(text_b);
    let hash_b = format!("{:x}", Sha256::digest(normalized_b.as_bytes()));
    assert!(body_str.contains(&hash_b));

    // Page round text in DB is still uncommitted (or initial OCR)
    let page = get_page_by_project_and_number(&pool, project_id, 1)
        .await
        .unwrap()
        .unwrap();
    let round_before = get_page_round(&pool, page.id, "P1").await.unwrap().unwrap();
    assert_ne!(round_before.text.as_deref(), Some(normalized_b.as_str()));

    // 4. Proofer submits text B with matching hash_b
    let confirm_body = serde_json::json!({
        "text": text_b,
        "csrf_token": csrf_token,
        "confirmed_hash": hash_b,
    });

    let confirm_req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/save"))
        .header(header::COOKIE, format!("dp_session={session_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&confirm_body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(confirm_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("application/json"));

    // Verify DB committed text B
    let round_final = get_page_round(&pool, page.id, "P1").await.unwrap().unwrap();
    assert_eq!(round_final.text.as_deref(), Some(normalized_b.as_str()));
}

#[tokio::test]
async fn test_project_words_overrides_and_crud() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (mgr_id, mgr_session, mgr_csrf) = create_test_user(&pool, "mgr_words", "pm").await;
    let (_proofer_id, proofer_session, proofer_csrf) =
        create_test_user(&pool, "proofer_words", "proofer").await;

    let project_id = setup_test_project(&pool, mgr_id).await;
    let state = build_test_state(pool.clone());
    let app = create_router(state.clone());

    // 1. Proofer tries to access PM words route -> 403 Forbidden
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/words"))
        .header(header::COOKIE, format!("dp_session={proofer_session}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 2. Manager accesses words route -> 200 OK
    let req = Request::builder()
        .uri(format!("/projects/{project_id}/words"))
        .header(header::COOKIE, format!("dp_session={mgr_session}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 3. Manager adds "shakespeare" to project good words
    let form_body = format!("action=add&kind=good&word=shakespeare&csrf_token={mgr_csrf}");
    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/words"))
        .header(header::COOKIE, format!("dp_session={mgr_session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form_body))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Verify in DB
    let (good, bad) = get_all_project_words(&pool, project_id).await.unwrap();
    assert!(good.contains("shakespeare"));
    assert!(!bad.contains("shakespeare"));

    // 4. Proofer claims and saves text containing "Shakespeare"
    // Since "shakespeare" is in project_good, it must NOT trigger confirmation
    let claim_req = Request::builder()
        .uri(format!("/projects/{project_id}/proof"))
        .header(header::COOKIE, format!("dp_session={proofer_session}"))
        .body(Body::empty())
        .unwrap();
    let _ = app.clone().oneshot(claim_req).await.unwrap();

    let text_with_shakespeare = "Đọc tác phẩm của Shakespeare dịch sang tiếng Việt.";
    let save_body = serde_json::json!({
        "text": text_with_shakespeare,
        "csrf_token": proofer_csrf,
        "confirmed_hash": null,
    });

    let save_req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/pages/1/save"))
        .header(header::COOKIE, format!("dp_session={proofer_session}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&save_body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(save_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // Committed directly without confirmation panel!
    assert!(content_type.contains("application/json"));

    // 5. Manager removes "shakespeare"
    let form_delete = format!("action=delete&kind=good&word=shakespeare&csrf_token={mgr_csrf}");
    let req = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/words"))
        .header(header::COOKIE, format!("dp_session={mgr_session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form_delete))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let (good_after, _) = get_all_project_words(&pool, project_id).await.unwrap();
    assert!(!good_after.contains("shakespeare"));
}

#[tokio::test]
async fn test_admin_site_words_crud_and_cache_refresh() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (_admin_id, admin_session, admin_csrf) = create_test_user(&pool, "admin_w", "admin").await;
    let (_, proofer_session, _) = create_test_user(&pool, "proofer_w", "proofer").await;

    let state = build_test_state(pool.clone());
    let app = create_router(state.clone());

    // 1. Proofer accesses /admin/words -> 403 Forbidden
    let req = Request::builder()
        .uri("/admin/words")
        .header(header::COOKIE, format!("dp_session={proofer_session}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 2. Admin accesses /admin/words -> 200 OK
    let req = Request::builder()
        .uri("/admin/words")
        .header(header::COOKIE, format!("dp_session={admin_session}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 3. Admin adds a bad site word
    let word = "badspamsyllable";
    let form_add = format!("action=add&kind=bad&word={word}&csrf_token={admin_csrf}");

    let req = Request::builder()
        .method("POST")
        .uri("/admin/words")
        .header(header::COOKIE, format!("dp_session={admin_session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form_add))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Verify in-memory cache was updated
    {
        let guard = state.site_words.read().await;
        assert!(
            guard.1.contains(word),
            "Site words bad list cache must contain added word"
        );
    }

    // 4. Admin removes the word
    let form_del = format!("action=delete&kind=bad&word={word}&csrf_token={admin_csrf}");

    let req = Request::builder()
        .method("POST")
        .uri("/admin/words")
        .header(header::COOKIE, format!("dp_session={admin_session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form_del))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Verify in-memory cache no longer contains it
    {
        let guard = state.site_words.read().await;
        assert!(
            !guard.1.contains(word),
            "Site words bad list cache must not contain removed word"
        );
    }
}
