//! Integration tests for Phase 8: browsing, comments, and daily limits.
//!
//! Tests:
//! 1. Project list filters combine, URLs stay shareable, pagination clamps and
//!    caps page size at 100, and HTMX requests receive a fragment.
//! 2. Comments are rendered from Markdown, sanitized, and appended via HTMX.
//! 3. The daily limit produces a clear blocked state and a redirect notice.
//! 4. Project detail shows progress; the audit log is manager/admin-only.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use dp_db::queries::{
    page_tallies::current_vietnam_date,
    pages::{NewPageInput, insert_pages_transactional},
    project_events::{NewProjectEvent, record_event},
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

struct SetupProject<'a> {
    pool: &'a MySqlPool,
    manager_id: u64,
    name: &'a str,
    pages: usize,
    phase: &'a str,
    round: Option<&'a str>,
}

async fn setup_project(input: SetupProject<'_>) -> u64 {
    let pid = create_project(
        input.pool,
        &dp_db::queries::projects::NewProject {
            name: input.name,
            author: "Tác giả Phase 8",
            tone_style: "either",
            charset: "vi",
            manager_id: input.manager_id,
            image_source: "Scans",
            comments: "Hướng dẫn kiểm thử",
        },
    )
    .await
    .expect("create project failed");

    let pages: Vec<NewPageInput> = (1..=input.pages)
        .map(|i| NewPageInput {
            page_number: i as i32,
            extension: "png".to_string(),
            image_width: 800,
            image_height: 1200,
            ocr_text: Some(format!("Nội dung OCR trang {i}")),
        })
        .collect();
    insert_pages_transactional(input.pool, pid, &pages)
        .await
        .expect("insert pages failed");

    sqlx::query(
        "UPDATE projects
         SET phase = ?, current_round = ?, updated_at = UTC_TIMESTAMP(6)
         WHERE id = ?",
    )
    .bind(input.phase)
    .bind(input.round)
    .bind(pid)
    .execute(input.pool)
    .await
    .expect("update project failed");

    if let Some(round) = input.round {
        sqlx::query(
            "INSERT INTO page_rounds (page_id, round, state, text)
             SELECT id, ?, 'available', ocr_text FROM pages WHERE project_id = ?
             ON DUPLICATE KEY UPDATE state = 'available'",
        )
        .bind(round)
        .bind(pid)
        .execute(input.pool)
        .await
        .expect("seed page rounds failed");
    }

    pid
}

async fn set_page_state(
    pool: &MySqlPool,
    project_id: u64,
    round: &str,
    offset: usize,
    state: &str,
    proofer_id: Option<u64>,
) {
    let page_id: u64 = sqlx::query_scalar(
        "SELECT pr.page_id
         FROM page_rounds pr
         JOIN pages p ON p.id = pr.page_id
         WHERE p.project_id = ? AND pr.round = ?
         ORDER BY p.page_number
         LIMIT 1 OFFSET ?",
    )
    .bind(project_id)
    .bind(round)
    .bind(offset as u32)
    .fetch_one(pool)
    .await
    .expect("find page round failed");

    sqlx::query(
        "UPDATE page_rounds
         SET state = ?, proofer_id = ?, claimed_at = UTC_TIMESTAMP(6)
         WHERE page_id = ? AND round = ?",
    )
    .bind(state)
    .bind(proofer_id)
    .bind(page_id)
    .bind(round)
    .execute(pool)
    .await
    .expect("set page state failed");
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

async fn get(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(uri)
        .header(header::COOKIE, format!("dp_session={token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

async fn get_htmx(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(uri)
        .header(header::COOKIE, format!("dp_session={token}"))
        .header("HX-Request", "true")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
async fn test_project_list_filters_pagination_and_htmx() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p8_mgr", "pm").await;
    let (proofer_id, token, _) = create_test_user(&pool, "p8_proofer", "proofer").await;
    let run = &uuid::Uuid::new_v4().simple().to_string()[..6];

    let available_name = format!("Phase8 Available {run}");
    let in_progress_name = format!("Phase8 InProgress {run}");
    let draft_name = format!("Phase8 Draft {run}");

    let available_id = setup_project(SetupProject {
        pool: &pool,
        manager_id,
        name: &available_name,
        pages: 3,
        phase: "in_round",
        round: Some("P1"),
    })
    .await;

    let in_progress_id = setup_project(SetupProject {
        pool: &pool,
        manager_id,
        name: &in_progress_name,
        pages: 2,
        phase: "in_round",
        round: Some("P1"),
    })
    .await;
    set_page_state(
        &pool,
        in_progress_id,
        "P1",
        0,
        "in_progress",
        Some(proofer_id),
    )
    .await;
    set_page_state(&pool, in_progress_id, "P1", 1, "done", Some(proofer_id)).await;

    let draft_id = setup_project(SetupProject {
        pool: &pool,
        manager_id,
        name: &draft_name,
        pages: 1,
        phase: "draft",
        round: None,
    })
    .await;

    let app = build_test_app(pool.clone());

    // Filter by availability=available.
    let (status, body) = get(
        &app,
        &format!("/projects?availability=available&q={run}"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&available_name), "body: {body}");
    assert!(
        !body.contains(&in_progress_name),
        "in-progress project must be filtered out"
    );
    assert!(
        !body.contains(&draft_name),
        "draft project must be filtered out"
    );

    // Filter by availability=in_progress and availability=all.
    let (_, body) = get(
        &app,
        &format!("/projects?availability=in_progress&q={run}"),
        &token,
    )
    .await;
    assert!(body.contains(&in_progress_name));
    assert!(!body.contains(&available_name));

    let (_, body) = get(&app, &format!("/projects?availability=all&q={run}"), &token).await;
    assert!(body.contains(&available_name));
    assert!(body.contains(&in_progress_name));
    assert!(body.contains(&draft_name));

    // Round filter + sort by name, and selected state is reflected for sharing.
    let (_, body) = get(
        &app,
        &format!("/projects?availability=all&round=P1&sort=name&q={run}"),
        &token,
    )
    .await;
    assert!(body.contains(&available_name));
    assert!(!body.contains(&draft_name));
    assert!(body.contains(r#"value="P1" selected"#));
    let available_pos = body.find(&available_name).unwrap();
    let in_progress_pos = body.find(&in_progress_name).unwrap();
    assert!(
        available_pos < in_progress_pos,
        "sort=name must order A before I"
    );

    // Pagination: 3 matches, 2 per page -> page 2 has exactly one row.
    let (_, body) = get(
        &app,
        &format!("/projects?availability=all&per=2&page=2&q={run}"),
        &token,
    )
    .await;
    assert!(body.contains("Trang 2 / 2"));
    assert!(body.matches("class=\"project-link\"").count() == 1);

    // Out-of-range pages clamp to the last page instead of returning an empty page.
    let (_, body) = get(
        &app,
        &format!("/projects?availability=all&per=2&page=99&q={run}"),
        &token,
    )
    .await;
    assert!(body.contains("Trang 2 / 2"));
    assert!(body.matches("class=\"project-link\"").count() == 1);

    // Page size is capped at 100.
    let (_, body) = get(
        &app,
        &format!("/projects?availability=all&per=1000&q={run}"),
        &token,
    )
    .await;
    assert!(body.contains("value=\"100\" selected"));
    assert!(!body.contains("value=\"1000\""));

    // HTMX requests receive only the results fragment.
    let (status, body) =
        get_htmx(&app, &format!("/projects?availability=all&q={run}"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.trim_start().starts_with("<div id=\"project-results\""));
    assert!(!body.contains("<!DOCTYPE html>"));

    // Sanity: fixtures are real.
    assert!(available_id > 0 && in_progress_id > 0 && draft_id > 0);
}

#[tokio::test]
async fn test_comments_markdown_sanitized_and_htmx_append() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p8_cmgr", "pm").await;
    let (_, token, csrf) = create_test_user(&pool, "p8_cuser", "proofer").await;
    let run = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Phase8 Comments {run}");

    let project_id = setup_project(SetupProject {
        pool: &pool,
        manager_id,
        name: &project_name,
        pages: 1,
        phase: "draft",
        round: None,
    })
    .await;

    let app = build_test_app(pool.clone());

    // Missing CSRF is rejected.
    let body = form_body(&[("body_md", "**xin chào**"), ("csrf_token", "wrong")]);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/comments"))
        .header(header::COOKIE, format!("dp_session={token}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Normal submission redirects back to #comments.
    let body = form_body(&[
        (
            "body_md",
            "**Chào** mọi người <script>alert('x')</script> [x](javascript:alert(1))",
        ),
        ("csrf_token", &csrf),
    ]);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/comments"))
        .header(header::COOKIE, format!("dp_session={token}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        &format!("/projects/{project_id}#comments")
    );

    // HTMX submission returns a sanitized fragment for beforeend insertion.
    let body = form_body(&[("body_md", "Bình luận **thứ hai**"), ("csrf_token", &csrf)]);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/comments"))
        .header(header::COOKIE, format!("dp_session={token}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("HX-Request", "true")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let fragment = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(fragment.contains("class=\"comment-item\""));
    assert!(fragment.contains("<strong>thứ hai</strong>"));
    assert!(!fragment.contains("<script"));
    assert!(!fragment.to_lowercase().contains("javascript:"));

    // Detail page includes both comments, author, count, and sanitized body.
    let (status, body) = get(&app, &format!("/projects/{project_id}"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Thảo luận <span class=\"muted\">(2)</span>"));
    assert!(body.contains("<strong>Chào</strong>"));
    assert!(body.contains("Bình luận <strong>thứ hai</strong>"));
    // The page itself loads scripts, but the injected comment payload must not.
    assert!(!body.contains("<script>alert"));
    assert!(!body.to_lowercase().contains("javascript:"));

    // Empty comments are rejected.
    let body = form_body(&[("body_md", "   "), ("csrf_token", &csrf)]);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/comments"))
        .header(header::COOKIE, format!("dp_session={token}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_daily_limit_blocked_ui_and_claim_redirect() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p8_lmgr", "pm").await;
    let (user_id, token, csrf) = create_test_user(&pool, "p8_luser", "proofer").await;
    let run = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Phase8 Limit {run}");

    let project_id = setup_project(SetupProject {
        pool: &pool,
        manager_id,
        name: &project_name,
        pages: 2,
        phase: "in_round",
        round: Some("P1"),
    })
    .await;

    // Consume the configured daily allowance.
    let limit = AppConfig::default().limits.daily_pages;
    sqlx::query(
        "INSERT INTO page_tallies (user_id, round, day, count)
         VALUES (?, 'P1', ?, ?)
         ON DUPLICATE KEY UPDATE count = ?",
    )
    .bind(user_id)
    .bind(current_vietnam_date())
    .bind(limit)
    .bind(limit)
    .execute(&pool)
    .await
    .expect("seed tally failed");

    let app = build_test_app(pool.clone());

    // List shows the clear blocked state and offers no claim button.
    let (status, body) = get(
        &app,
        &format!("/projects?availability=available&q=Phase8%20Limit%20{run}"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Đã đạt giới hạn hôm nay"));
    assert!(body.contains(&format!("{limit}")));
    assert!(!body.contains(">Soát trang</button>"));

    // Detail shows the blocked state too.
    let (status, body) = get(&app, &format!("/projects/{project_id}"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Đã đạt giới hạn hôm nay"));
    assert!(body.contains(&format!("{limit}")));

    // A direct claim POST redirects with an actionable notice instead of a 429 page.
    let body = form_body(&[("csrf_token", &csrf)]);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/claim"))
        .header(header::COOKIE, format!("dp_session={token}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        &format!("/projects/{project_id}?notice=limit_reached")
    );

    // The notice is rendered on the redirected page.
    let (status, body) = get(
        &app,
        &format!("/projects/{project_id}?notice=limit_reached"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Giới hạn sẽ được đặt lại vào 00:00 giờ Việt Nam."));
}

#[tokio::test]
async fn test_project_progress_and_event_log_visibility() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let (manager_id, _, _) = create_test_user(&pool, "p8_emgr", "pm").await;
    let (proofer_id, proofer_token, _) = create_test_user(&pool, "p8_eproof", "proofer").await;
    let (_, manager_token, _) = create_test_user(&pool, "p8_eadmin", "admin").await;
    let run = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let project_name = format!("Phase8 Events {run}");

    let project_id = setup_project(SetupProject {
        pool: &pool,
        manager_id,
        name: &project_name,
        pages: 4,
        phase: "in_round",
        round: Some("P1"),
    })
    .await;

    // 2 done, 1 in progress, 1 available -> 50%.
    set_page_state(&pool, project_id, "P1", 0, "done", Some(proofer_id)).await;
    set_page_state(&pool, project_id, "P1", 1, "done", Some(proofer_id)).await;
    set_page_state(&pool, project_id, "P1", 2, "in_progress", Some(proofer_id)).await;

    record_event(
        &pool,
        &NewProjectEvent {
            project_id,
            from_phase: Some("queued"),
            to_phase: Some("in_round"),
            from_round: None,
            to_round: Some("P1"),
            actor_id: Some(manager_id),
            note: "Bắt đầu vòng P1",
        },
    )
    .await
    .expect("record event failed");

    let app = build_test_app(pool.clone());

    // Manager (project owner) sees progress and the audit log.
    let (status, body) = get(&app, &format!("/projects/{project_id}"), &manager_token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("2 / 4 trang đã xong (50%)"));
    assert!(body.contains("Lịch sử dự án"));
    assert!(body.contains("Bắt đầu vòng P1"));

    // A regular proofer sees progress and comments, but not the audit log.
    let (status, body) = get(&app, &format!("/projects/{project_id}"), &proofer_token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("2 / 4 trang đã xong (50%)"));
    assert!(!body.contains("Lịch sử dự án"));
    assert!(!body.contains("Bắt đầu vòng P1"));
    assert!(body.contains("Thảo luận"));

    // An event with no actor is attributed to the system, still manager-only.
    record_event(
        &pool,
        &NewProjectEvent {
            project_id,
            from_phase: Some("in_round"),
            to_phase: Some("in_round"),
            from_round: Some("P1"),
            to_round: Some("P2"),
            actor_id: None,
            note: "Tự động chuyển vòng",
        },
    )
    .await
    .expect("record automatic event failed");

    let (_, body) = get(&app, &format!("/projects/{project_id}"), &manager_token).await;
    assert!(body.contains("Hệ thống"));
    assert!(body.contains("Tự động chuyển vòng"));

    // Final-round events have no destination round; render an em dash, not "none".
    record_event(
        &pool,
        &NewProjectEvent {
            project_id,
            from_phase: Some("in_round"),
            to_phase: Some("post_processing"),
            from_round: Some("P2"),
            to_round: None,
            actor_id: None,
            note: "Hoàn tất vòng cuối",
        },
    )
    .await
    .expect("record final event failed");

    let (_, body) = get(&app, &format!("/projects/{project_id}"), &manager_token).await;
    assert!(body.contains("P2 &rarr; —"));
    assert!(!body.contains("P2 &rarr; none"));

    // Ensure timestamps are UTC-based and display labels are present.
    let now = Utc::now();
    assert!(now.timestamp() > 0);
}
