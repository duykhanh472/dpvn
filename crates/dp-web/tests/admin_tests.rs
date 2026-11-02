//! Integration tests for Phase 10: administration.
//!
//! Covers access control, role changes taking effect on the next request,
//! last-active-admin protection, destructive-action confirmation, runtime
//! settings, project oversight, and the audit trail.
//!
//! Tests share one MySQL schema, so they serialize on [`ADMIN_LOCK`] and any
//! global state they touch (settings, other admins' status) is restored.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use dp_db::queries::{
    pages::{NewPageInput, insert_pages_transactional},
    projects::create_project,
    sessions::create_session,
    settings::{delete_setting, get_setting, upsert_setting},
    users::{get_user_roles, grant_user_role, revoke_user_role_guarded, set_user_status_guarded},
};
use dp_web::{AppConfig, AppState, AssetManifest, RateLimiter, create_router, create_template_env};
use sqlx::MySqlPool;
use std::sync::Arc;
use tower::ServiceExt;

/// Serialize tests that mutate global admin/settings state.
static ADMIN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn get_test_pool() -> Option<MySqlPool> {
    let db_url = std::env::var("DP_DATABASE_URL")
        .unwrap_or_else(|_| "mysql://dp:secret@127.0.0.1:3306/dp".to_string());

    let pool = dp_db::create_pool(&db_url, 1, 10).await.ok()?;
    dp_db::run_migrations(&pool).await.ok()?;
    Some(pool)
}

async fn create_test_user(pool: &MySqlPool, prefix: &str, roles: &[&str]) -> (u64, String, String) {
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

    for role in roles {
        sqlx::query(
            "INSERT INTO user_roles (user_id, role, granted_at) VALUES (?, ?, UTC_TIMESTAMP(6))",
        )
        .bind(uid)
        .bind(role)
        .execute(pool)
        .await
        .expect("grant role failed");
    }

    let raw_token = dp_web::auth::generate_token();
    let token_hash = dp_web::auth::hash_token(&raw_token);
    let csrf_token = dp_web::auth::generate_token();

    create_session(pool, &token_hash, uid, &csrf_token, None, "test-agent")
        .await
        .expect("create session failed");

    (uid, raw_token, csrf_token)
}

async fn create_test_project(pool: &MySqlPool, manager_id: u64) -> u64 {
    let rand = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let name = format!("Dự án Quản trị {rand}");
    let pid = create_project(
        pool,
        &dp_db::queries::projects::NewProject {
            name: &name,
            author: "Tác giả Quản trị",
            tone_style: "either",
            charset: "vi",
            manager_id,
            image_source: "Scans",
            comments: "Ghi chú quản trị",
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
            image_width: 800,
            image_height: 1200,
            ocr_text: Some("Nội dung OCR".to_string()),
        }],
    )
    .await
    .expect("insert pages failed");

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

async fn get(app: &axum::Router, uri: &str, session: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(session) = session {
        builder = builder.header(header::COOKIE, format!("dp_session={session}"));
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post_form(
    app: &axum::Router,
    uri: &str,
    session: &str,
    body: String,
) -> axum::response::Response {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::COOKIE, format!("dp_session={session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn audit_count(pool: &MySqlPool, action: &str, target_id: Option<u64>) -> i64 {
    let count: (i64,) = if let Some(target_id) = target_id {
        sqlx::query_as("SELECT COUNT(*) FROM admin_audit_log WHERE action = ? AND target_id = ?")
            .bind(action)
            .bind(target_id)
            .fetch_one(pool)
            .await
            .expect("audit count failed")
    } else {
        sqlx::query_as("SELECT COUNT(*) FROM admin_audit_log WHERE action = ?")
            .bind(action)
            .fetch_one(pool)
            .await
            .expect("audit count failed")
    };
    count.0
}

async fn user_status(pool: &MySqlPool, user_id: u64) -> String {
    let (status,): (String,) = sqlx::query_as("SELECT status FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("user status query failed");
    status
}

async fn project_phase_and_hold(pool: &MySqlPool, project_id: u64) -> (String, i8) {
    let row: (String, i8) = sqlx::query_as("SELECT phase, on_hold FROM projects WHERE id = ?")
        .bind(project_id)
        .fetch_one(pool)
        .await
        .expect("project query failed");
    row
}

/// Disable every active admin except `keep`, returning the IDs changed.
async fn disable_other_active_admins(pool: &MySqlPool, keep: u64) -> Vec<u64> {
    let rows: Vec<(u64,)> = sqlx::query_as(
        "SELECT ur.user_id
         FROM user_roles ur
         JOIN users u ON u.id = ur.user_id
         WHERE ur.role = 'admin' AND u.status = 'active' AND ur.user_id <> ?",
    )
    .bind(keep)
    .fetch_all(pool)
    .await
    .expect("list active admins failed");

    let ids: Vec<u64> = rows.into_iter().map(|(id,)| id).collect();
    if !ids.is_empty() {
        for id in &ids {
            sqlx::query("UPDATE users SET status = 'disabled' WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await
                .expect("disable admin failed");
        }
    }
    ids
}

async fn restore_active_users(pool: &MySqlPool, ids: &[u64]) {
    for id in ids {
        sqlx::query("UPDATE users SET status = 'active' WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .expect("restore user failed");
    }
}

#[tokio::test]
async fn test_admin_access_control() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (_admin_id, admin_session, _) = create_test_user(&pool, "admin_access", &["admin"]).await;
    let (_proofer_id, proofer_session, _) =
        create_test_user(&pool, "proofer_access", &["proofer"]).await;
    let app = create_router(build_test_state(pool.clone()));

    for uri in [
        "/admin",
        "/admin/users",
        "/admin/projects",
        "/admin/settings",
        "/admin/audit",
        "/admin/words",
    ] {
        let resp = get(&app, uri, Some(&admin_session)).await;
        assert_eq!(resp.status(), StatusCode::OK, "admin GET {uri}");
    }

    for uri in [
        "/admin",
        "/admin/users",
        "/admin/projects",
        "/admin/settings",
    ] {
        let resp = get(&app, uri, Some(&proofer_session)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "proofer GET {uri}");
    }

    let resp = get(&app, "/admin", None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_role_change_takes_effect_on_next_request() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (_admin_id, admin_session, admin_csrf) =
        create_test_user(&pool, "admin_roles", &["admin"]).await;
    let (proofer_id, proofer_session, _) =
        create_test_user(&pool, "proofer_roles", &["proofer"]).await;
    let app = create_router(build_test_state(pool.clone()));

    // A plain proofer cannot reach manager-only pages.
    let resp = get(&app, "/my/projects", Some(&proofer_session)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Grant the PM role; the very next request must see it.
    let body = format!("action=grant&role=pm&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{proofer_id}/roles"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = get(&app, "/my/projects", Some(&proofer_session)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(audit_count(&pool, "user.role.grant", Some(proofer_id)).await >= 1);

    // Revoke it (with confirmation); access must disappear immediately.
    let body = format!("action=revoke&role=pm&confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{proofer_id}/roles"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = get(&app, "/my/projects", Some(&proofer_session)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(audit_count(&pool, "user.role.revoke", Some(proofer_id)).await >= 1);
}

#[tokio::test]
async fn test_destructive_actions_require_confirmation_and_are_audited() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (_admin_id, admin_session, admin_csrf) =
        create_test_user(&pool, "admin_confirm", &["admin"]).await;
    let (target_id, _target_session, _) =
        create_test_user(&pool, "target_confirm", &["proofer", "pm"]).await;
    let app = create_router(build_test_state(pool.clone()));

    // Revoking a role without `confirm` is rejected and changes nothing.
    let body = format!("action=revoke&role=pm&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{target_id}/roles"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        get_user_roles(&pool, target_id)
            .await
            .unwrap()
            .contains(&"pm".to_string())
    );

    // Disabling an account without `confirm` is rejected.
    let body = format!("status=disabled&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{target_id}/status"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(user_status(&pool, target_id).await, "active");

    // Confirmed destructive actions succeed and are audited.
    let body = format!("action=revoke&role=pm&confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{target_id}/roles"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let body = format!("status=disabled&confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{target_id}/status"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(user_status(&pool, target_id).await, "disabled");
    assert!(audit_count(&pool, "user.status", Some(target_id)).await >= 1);
}

#[tokio::test]
async fn test_last_active_admin_is_protected() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (admin_a, _session_a, _) = create_test_user(&pool, "last_admin_a", &["admin"]).await;
    let (admin_b, _session_b, _) = create_test_user(&pool, "last_admin_b", &["admin"]).await;

    // Leave exactly one active administrator (admin_b).
    let disabled = disable_other_active_admins(&pool, admin_b).await;

    let (active_admins,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM user_roles ur
         JOIN users u ON u.id = ur.user_id
         WHERE ur.role = 'admin' AND u.status = 'active'",
    )
    .fetch_one(&pool)
    .await
    .expect("count active admins failed");

    let revoke_result = revoke_user_role_guarded(&pool, admin_b, "admin").await;
    let disable_result = set_user_status_guarded(&pool, admin_b, "disabled").await;

    restore_active_users(&pool, &disabled).await;
    // Ensure admin_a remains an active admin for later runs.
    let _ = grant_user_role(&pool, admin_a, "admin").await;

    assert_eq!(active_admins, 1);

    assert!(
        matches!(revoke_result, Err(dp_db::DbError::Conflict(_))),
        "revoking the last admin role must be rejected"
    );
    assert!(
        matches!(disable_result, Err(dp_db::DbError::Conflict(_))),
        "disabling the last active admin must be rejected"
    );
    assert_eq!(user_status(&pool, admin_b).await, "active");
    assert!(
        get_user_roles(&pool, admin_b)
            .await
            .unwrap()
            .contains(&"admin".to_string())
    );

    // HTTP path: an admin cannot revoke their own last admin role.
    let (solo_admin, solo_session, solo_csrf) =
        create_test_user(&pool, "last_admin_http", &["admin"]).await;
    let disabled = disable_other_active_admins(&pool, solo_admin).await;

    let app = create_router(build_test_state(pool.clone()));
    let body = format!("action=revoke&role=admin&confirm=1&csrf_token={solo_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/users/{solo_admin}/roles"),
        &solo_session,
        body,
    )
    .await;
    restore_active_users(&pool, &disabled).await;

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(
        get_user_roles(&pool, solo_admin)
            .await
            .unwrap()
            .contains(&"admin".to_string())
    );
}

#[tokio::test]
async fn test_site_word_delete_requires_confirmation_and_is_audited() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (_admin_id, admin_session, admin_csrf) =
        create_test_user(&pool, "admin_site_words", &["admin"]).await;
    let app = create_router(build_test_state(pool.clone()));
    let word = format!("auditword_{}", uuid::Uuid::new_v4().simple());

    let body = format!("action=add&kind=bad&word={word}&csrf_token={admin_csrf}");
    let resp = post_form(&app, "/admin/words", &admin_session, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(audit_count(&pool, "site_word.add", None).await >= 1);

    // Deleting without confirmation is rejected and keeps the word.
    let body = format!("action=delete&kind=bad&word={word}&csrf_token={admin_csrf}");
    let resp = post_form(&app, "/admin/words", &admin_session, body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let (_, bad) = dp_db::queries::words::get_all_site_words(&pool)
        .await
        .unwrap();
    assert!(bad.contains(&word));

    // Confirmed deletion succeeds and is audited.
    let body = format!("action=delete&kind=bad&word={word}&confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(&app, "/admin/words", &admin_session, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let (_, bad) = dp_db::queries::words::get_all_site_words(&pool)
        .await
        .unwrap();
    assert!(!bad.contains(&word));
    assert!(audit_count(&pool, "site_word.delete", None).await >= 1);
}

#[tokio::test]
async fn test_settings_control_registration_and_limits() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let previous_registration = get_setting(&pool, "registration_open").await.unwrap();
    let previous_daily = get_setting(&pool, "daily_pages").await.unwrap();
    let previous_timeout = get_setting(&pool, "claim_timeout_hours").await.unwrap();

    let (_admin_id, admin_session, admin_csrf) =
        create_test_user(&pool, "admin_settings", &["admin"]).await;
    let app = create_router(build_test_state(pool.clone()));

    // Closing registration: omit the checkbox field.
    let body = format!("daily_pages=7&claim_timeout_hours=5&csrf_token={admin_csrf}");
    let resp = post_form(&app, "/admin/settings", &admin_session, body).await;
    let save_status = resp.status();

    let page = get(&app, "/admin/settings?saved=1", Some(&admin_session)).await;
    let body = to_bytes(page.into_body(), usize::MAX).await.unwrap();
    let settings_html = String::from_utf8_lossy(&body).to_string();

    let audit_page = get(&app, "/admin/audit?tab=admin", Some(&admin_session)).await;
    let audit_status = audit_page.status();

    let page = get(&app, "/register", None).await;
    let body = to_bytes(page.into_body(), usize::MAX).await.unwrap();
    let register_html = String::from_utf8_lossy(&body).to_string();

    let body = format!(
        "username=closedreg_{}&email=closedreg_{}@example.com&password=Sup3rSecret!&real_name=Test",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple(),
    );
    let resp = post_form(&app, "/register", "", body).await;
    let register_status = resp.status();
    let audited = audit_count(&pool, "settings.update", None).await;

    // Restore previous settings before asserting so a failure cannot leave the
    // shared schema with registration closed.
    for (key, previous) in [
        ("registration_open", previous_registration),
        ("daily_pages", previous_daily),
        ("claim_timeout_hours", previous_timeout),
    ] {
        if let Some(value) = previous {
            upsert_setting(&pool, key, &value).await.unwrap();
        } else {
            delete_setting(&pool, key).await.unwrap();
        }
    }

    assert_eq!(save_status, StatusCode::SEE_OTHER);
    assert!(settings_html.contains("value=\"7\""));
    assert!(settings_html.contains("value=\"5\""));
    assert!(settings_html.contains("Đã lưu cài đặt thành công"));
    assert_eq!(audit_status, StatusCode::OK);
    assert!(register_html.contains("đang tạm đóng"));
    // No session cookie is needed for registration, but the closed check runs first.
    assert_eq!(register_status, StatusCode::FORBIDDEN);
    assert!(audited >= 1);
}

#[tokio::test]
async fn test_project_oversight_transition_hold_and_audit() {
    let _guard = ADMIN_LOCK.lock().await;
    let Some(pool) = get_test_pool().await else {
        eprintln!("Database not available, skipping test");
        return;
    };

    let (admin_id, admin_session, admin_csrf) =
        create_test_user(&pool, "admin_projects", &["admin"]).await;
    let project_id = create_test_project(&pool, admin_id).await;
    let app = create_router(build_test_state(pool.clone()));

    // Confirmation is mandatory before any forced transition.
    let body = format!("target_phase=queued&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/projects/{project_id}/transition"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(project_phase_and_hold(&pool, project_id).await.0, "draft");

    // Illegal transitions follow the domain state machine.
    let body = format!("target_phase=archived&confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/projects/{project_id}/transition"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // A legal forced transition succeeds and is recorded twice: domain event + admin audit.
    let body = format!("target_phase=queued&confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/projects/{project_id}/transition"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(project_phase_and_hold(&pool, project_id).await.0, "queued");
    assert!(audit_count(&pool, "project.transition", Some(project_id)).await >= 1);

    let events: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM project_events WHERE project_id = ?")
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(events.0 >= 1);

    let audit_page = get(&app, "/admin/audit?tab=projects", Some(&admin_session)).await;
    assert_eq!(audit_page.status(), StatusCode::OK);

    // Pausing requires confirmation; resuming does not.
    let body = format!("csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/projects/{project_id}/hold"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(project_phase_and_hold(&pool, project_id).await.1, 0);

    let body = format!("confirm=1&csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/projects/{project_id}/hold"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(project_phase_and_hold(&pool, project_id).await.1, 1);
    assert!(audit_count(&pool, "project.hold", Some(project_id)).await >= 1);

    let body = format!("csrf_token={admin_csrf}");
    let resp = post_form(
        &app,
        &format!("/admin/projects/{project_id}/hold"),
        &admin_session,
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(project_phase_and_hold(&pool, project_id).await.1, 0);
}
