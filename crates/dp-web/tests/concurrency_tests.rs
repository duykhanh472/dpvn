//! Concurrency and state-machine integration tests per Section 6.7.
//!
//! Mandatory acceptance tests:
//! 1. Double claim: 2 tasks claim the same page -> exactly 1 succeeds.
//! 2. Last-page race: 2 tasks save the last 2 pages of P1 concurrently -> advances to P2 exactly once.
//! 3. Duplicate save: same page saved twice -> second returns 409 conflict, tally increments once.
//! 4. Own-page rule: user claims a page they proofed in P1 (now in P2) -> candidate query excludes it.
//! 5. Deadlock test: 20 concurrent saves across 5 projects -> no deadlock, all commit cleanly.
//! 6. Scripted full run: draft -> queued -> in_round(P1) -> in_round(P2).

use chrono::Utc;
use dp_core::round::RoundsConfig;
use dp_core::{ActorRole, Phase, UserId};
use dp_db::queries::{
    page_rounds::{
        SavePageInput, claim_next_available_page, get_page_round, list_project_page_rounds,
        save_page,
    },
    page_tallies::{check_daily_limit, current_vietnam_date, get_daily_tally},
    pages::{NewPageInput, insert_pages_transactional},
    project_events::list_project_events,
    projects::{create_project, get_project_by_id, transition_project},
    users::create_admin_user,
};
use sqlx::MySqlPool;
use std::sync::Arc;

async fn get_test_pool() -> Option<MySqlPool> {
    let db_url = std::env::var("DP_DATABASE_URL")
        .unwrap_or_else(|_| "mysql://dp:secret@127.0.0.1:3306/dp".to_string());

    let pool = dp_db::create_pool(&db_url, 4, 32).await.ok()?;
    dp_db::run_migrations(&pool).await.ok()?;
    Some(pool)
}

async fn create_test_user(pool: &MySqlPool, prefix: &str) -> u64 {
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

    sqlx::query("INSERT INTO user_roles (user_id, role, granted_at) VALUES (?, 'proofer', UTC_TIMESTAMP(6))")
        .bind(uid)
        .execute(pool)
        .await
        .expect("grant role failed");

    uid
}

async fn create_test_manager(pool: &MySqlPool, prefix: &str) -> u64 {
    let rand_id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let username = format!("{prefix}_{rand_id}");
    let email = format!("{username}@example.com");
    let hash = "$argon2id$v=19$m=19456,t=2,p=1$fake$hash";

    create_admin_user(pool, &username, &email, hash)
        .await
        .expect("create admin user failed")
}

async fn setup_project_in_round(
    pool: &MySqlPool,
    manager_id: u64,
    name: &str,
    num_pages: i32,
    round: &str,
) -> (u64, Vec<u64>) {
    let project_id = create_project(
        pool,
        &dp_db::queries::projects::NewProject {
            name,
            author: "Tác giả test",
            tone_style: "either",
            charset: "vi",
            manager_id,
            image_source: "Scans",
            comments: "Ghi chú kiểm thử",
        },
    )
    .await
    .expect("create project failed");

    let mut page_inputs = Vec::with_capacity(num_pages as usize);
    for i in 1..=num_pages {
        page_inputs.push(NewPageInput {
            page_number: i,
            extension: "png".to_string(),
            image_width: 800,
            image_height: 1200,
            ocr_text: Some(format!("Nội dung OCR trang {i}")),
        });
    }

    let inserted = insert_pages_transactional(pool, project_id, &page_inputs)
        .await
        .expect("insert pages failed");

    // Move project to in_round
    sqlx::query(
        "UPDATE projects
         SET phase = 'in_round', current_round = ?, updated_at = UTC_TIMESTAMP(6)
         WHERE id = ?",
    )
    .bind(round)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("update project to in_round failed");

    // Ensure page_rounds are available for this round
    let page_ids: Vec<u64> = inserted.iter().map(|p| p.page_id).collect();
    for &pid in &page_ids {
        sqlx::query(
            "INSERT INTO page_rounds (page_id, round, state, text)
             VALUES (?, ?, 'available', NULL)
             ON DUPLICATE KEY UPDATE state = 'available'",
        )
        .bind(pid)
        .bind(round)
        .execute(pool)
        .await
        .expect("seed page_rounds failed");
    }

    (project_id, page_ids)
}

// ───────────────────────────────────────────────────────────────────────────────
// Test 1: Double claim (§6.7)
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrency_double_claim() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let mgr = create_test_manager(&pool, "mgr_dc").await;
    let u1 = create_test_user(&pool, "u1_dc").await;
    let u2 = create_test_user(&pool, "u2_dc").await;

    // Create project with exactly 1 available page in P1
    let (pid, page_ids) = setup_project_in_round(&pool, mgr, "Double Claim Project", 1, "P1").await;
    let target_page_id = page_ids[0];

    let pool_arc = Arc::new(pool.clone());
    let (p1, p2) = (pool_arc.clone(), pool_arc.clone());

    // Launch two concurrent claim tasks
    let task1 =
        tokio::spawn(async move { claim_next_available_page(&p1, pid, "P1", None, u1, 0).await });
    let task2 =
        tokio::spawn(async move { claim_next_available_page(&p2, pid, "P1", None, u2, 0).await });

    let (res1, res2) = tokio::join!(task1, task2);
    let claim1 = res1.unwrap();
    let claim2 = res2.unwrap();

    let success1 = matches!(claim1, Ok(Some(_)));
    let success2 = matches!(claim2, Ok(Some(_)));

    assert!(
        (success1 && !success2) || (!success1 && success2),
        "exactly one task must succeed in claiming the single available page: res1={:?}, res2={:?}",
        claim1,
        claim2
    );

    // Verify database row state
    let pr = get_page_round(&pool, target_page_id, "P1")
        .await
        .unwrap()
        .expect("page_round must exist");
    assert_eq!(pr.state, "in_progress");
    let winner = if success1 { u1 } else { u2 };
    assert_eq!(pr.proofer_id, Some(winner));
}

// ───────────────────────────────────────────────────────────────────────────────
// Test 2: Last-page race (§6.7)
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrency_last_page_race() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let rounds_config = RoundsConfig::default_embedded();
    let mgr = create_test_manager(&pool, "mgr_lpr").await;
    let u1 = create_test_user(&pool, "u1_lpr").await;
    let u2 = create_test_user(&pool, "u2_lpr").await;

    // Create project with 2 pages in P1
    let (pid, page_ids) =
        setup_project_in_round(&pool, mgr, "Last Page Race Project", 2, "P1").await;
    let (page1, page2) = (page_ids[0], page_ids[1]);

    // Claim page1 by u1 and page2 by u2
    let now = Utc::now();
    sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
        .bind(u1).bind(now).bind(page1).execute(&pool).await.unwrap();
    sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
        .bind(u2).bind(now).bind(page2).execute(&pool).await.unwrap();

    let pool_arc = Arc::new(pool.clone());
    let rounds_arc = Arc::new(rounds_config);

    let (p1, r1) = (pool_arc.clone(), rounds_arc.clone());
    let (p2, r2) = (pool_arc.clone(), rounds_arc.clone());

    // Both proofers save simultaneously (each saves their own page)
    let save1 = tokio::spawn(async move {
        save_page(
            &p1,
            SavePageInput {
                project_id: pid,
                page_id: page1,
                round: "P1",
                user_id: u1,
                text: "Hoàn tất trang 1 bởi u1",
            },
            &r1,
        )
        .await
    });

    let save2 = tokio::spawn(async move {
        save_page(
            &p2,
            SavePageInput {
                project_id: pid,
                page_id: page2,
                round: "P1",
                user_id: u2,
                text: "Hoàn tất trang 2 bởi u2",
            },
            &r2,
        )
        .await
    });

    let (res1, res2) = tokio::join!(save1, save2);
    let outcome1 = res1.unwrap().expect("save1 must succeed");
    let outcome2 = res2.unwrap().expect("save2 must succeed");

    // Exactly one save should trigger the advance to P2
    let advanced_count = usize::from(outcome1.advanced_round.is_some())
        + usize::from(outcome2.advanced_round.is_some());
    assert_eq!(
        advanced_count, 1,
        "round advancement must trigger exactly once when last pages finish concurrently"
    );

    // Verify project current_round is now P2
    let proj = get_project_by_id(&pool, pid)
        .await
        .unwrap()
        .expect("project exists");
    assert_eq!(proj.phase, "in_round");
    assert_eq!(proj.current_round.as_deref(), Some("P2"));

    // Verify P2 has exactly 2 seeded pages in 'available' state
    let p2_pages = list_project_page_rounds(&pool, pid, "P2").await.unwrap();
    assert_eq!(p2_pages.len(), 2);
    for page in p2_pages {
        assert_eq!(page.state, "available");
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// Test 3: Duplicate save (§6.7)
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrency_duplicate_save() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let rounds_config = RoundsConfig::default_embedded();
    let mgr = create_test_manager(&pool, "mgr_dup").await;
    let u1 = create_test_user(&pool, "u1_dup").await;

    let (pid, page_ids) =
        setup_project_in_round(&pool, mgr, "Duplicate Save Project", 2, "P1").await;
    let page1 = page_ids[0];

    // u1 claims page1
    let now = Utc::now();
    sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
        .bind(u1).bind(now).bind(page1).execute(&pool).await.unwrap();

    let input = SavePageInput {
        project_id: pid,
        page_id: page1,
        round: "P1",
        user_id: u1,
        text: "Lưu trang lần 1",
    };

    // First save succeeds
    let res1 = save_page(&pool, input.clone(), &rounds_config).await;
    assert!(res1.is_ok(), "first save must succeed");

    // Second identical save must return 409 conflict because state is now 'done'
    let res2 = save_page(&pool, input, &rounds_config).await;
    assert!(
        matches!(res2, Err(dp_db::DbError::Conflict(_))),
        "second duplicate save must fail with conflict (409): {:?}",
        res2
    );

    // Tally must increment exactly once
    let today = current_vietnam_date();
    let tally = get_daily_tally(&pool, u1, "P1", today).await.unwrap();
    assert_eq!(
        tally, 1,
        "tally must be exactly 1 despite duplicate save attempt"
    );
}

// ───────────────────────────────────────────────────────────────────────────────
// Test 4: Own-page rule (§6.7)
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrency_own_page_rule() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let rounds_config = RoundsConfig::default_embedded();
    let mgr = create_test_manager(&pool, "mgr_opr").await;
    let u1 = create_test_user(&pool, "u1_opr").await;
    let u2 = create_test_user(&pool, "u2_opr").await;

    // Project with 2 pages
    let (pid, page_ids) =
        setup_project_in_round(&pool, mgr, "Own Page Rule Project", 2, "P1").await;
    let (page1, page2) = (page_ids[0], page_ids[1]);

    // u1 proofs page1 in P1, u2 proofs page2 in P1
    let now = Utc::now();
    sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
        .bind(u1).bind(now).bind(page1).execute(&pool).await.unwrap();
    sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
        .bind(u2).bind(now).bind(page2).execute(&pool).await.unwrap();

    save_page(
        &pool,
        SavePageInput {
            project_id: pid,
            page_id: page1,
            round: "P1",
            user_id: u1,
            text: "t1",
        },
        &rounds_config,
    )
    .await
    .unwrap();
    save_page(
        &pool,
        SavePageInput {
            project_id: pid,
            page_id: page2,
            round: "P1",
            user_id: u2,
            text: "t2",
        },
        &rounds_config,
    )
    .await
    .unwrap();

    // Now in P2. u1 tries to claim.
    // The candidate query MUST skip page1 (since u1 proofed it in P1) and assign page2!
    let claim_u1 = claim_next_available_page(&pool, pid, "P2", Some("P1"), u1, 1)
        .await
        .unwrap()
        .expect("u1 should find an available page");

    assert_eq!(
        claim_u1.page_id, page2,
        "own-page rule must prevent u1 from claiming page1 in P2; must receive page2 instead"
    );
}

// ───────────────────────────────────────────────────────────────────────────────
// Test 5: Deadlock test across multiple concurrent projects (§6.7)
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrency_deadlock_freedom() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let rounds_config = RoundsConfig::default_embedded();
    let mgr = create_test_manager(&pool, "mgr_dl").await;

    // Create 5 projects with 4 pages each = 20 total pages
    let mut tasks_data = Vec::new();
    for p_idx in 1..=5 {
        let (pid, page_ids) =
            setup_project_in_round(&pool, mgr, &format!("Deadlock Project {p_idx}"), 4, "P1").await;

        for page_id in page_ids {
            let proofer = create_test_user(&pool, &format!("pr_{p_idx}")).await;
            // Mark page in_progress
            let now = Utc::now();
            sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
                .bind(proofer).bind(now).bind(page_id).execute(&pool).await.unwrap();

            tasks_data.push((pid, page_id, proofer));
        }
    }

    assert_eq!(tasks_data.len(), 20);

    let pool_arc = Arc::new(pool.clone());
    let rounds_arc = Arc::new(rounds_config);
    let mut handles = Vec::new();

    // Spawn 20 concurrent saves across 5 projects
    for (pid, page_id, proofer) in tasks_data {
        let p = pool_arc.clone();
        let r = rounds_arc.clone();
        handles.push(tokio::spawn(async move {
            save_page(
                &p,
                SavePageInput {
                    project_id: pid,
                    page_id,
                    round: "P1",
                    user_id: proofer,
                    text: "Văn bản đã sửa bởi kiểm thử chống deadlock",
                },
                &r,
            )
            .await
        }));
    }

    let mut successes = 0;
    for h in handles {
        let res = h.await.unwrap();
        assert!(
            res.is_ok(),
            "all saves must succeed or cleanly retry without deadlocking: {:?}",
            res
        );
        successes += 1;
    }

    assert_eq!(successes, 20, "all 20 concurrent saves must commit cleanly");
}

// ───────────────────────────────────────────────────────────────────────────────
// Test: 100 iterations of last-page race without flake (Acceptance criterion)
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrency_100_iterations_no_flake() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let rounds_config = Arc::new(RoundsConfig::default_embedded());
    let pool_arc = Arc::new(pool.clone());
    let mgr = create_test_manager(&pool, "mgr_100").await;

    for i in 0..100 {
        let u1 = create_test_user(&pool, &format!("u1_{i}")).await;
        let u2 = create_test_user(&pool, &format!("u2_{i}")).await;

        let (pid, page_ids) =
            setup_project_in_round(&pool, mgr, &format!("Race 100 Iteration {i}"), 2, "P1").await;

        let (page1, page2) = (page_ids[0], page_ids[1]);
        let now = Utc::now();
        sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
            .bind(u1).bind(now).bind(page1).execute(&pool).await.unwrap();
        sqlx::query("UPDATE page_rounds SET state = 'in_progress', proofer_id = ?, claimed_at = ? WHERE page_id = ? AND round = 'P1'")
            .bind(u2).bind(now).bind(page2).execute(&pool).await.unwrap();

        let (p1, r1) = (pool_arc.clone(), rounds_config.clone());
        let (p2, r2) = (pool_arc.clone(), rounds_config.clone());

        let t1 = tokio::spawn(async move {
            save_page(
                &p1,
                SavePageInput {
                    project_id: pid,
                    page_id: page1,
                    round: "P1",
                    user_id: u1,
                    text: "t1",
                },
                &r1,
            )
            .await
        });

        let t2 = tokio::spawn(async move {
            save_page(
                &p2,
                SavePageInput {
                    project_id: pid,
                    page_id: page2,
                    round: "P1",
                    user_id: u2,
                    text: "t2",
                },
                &r2,
            )
            .await
        });

        let (res1, res2) = tokio::join!(t1, t2);
        let out1 = res1.unwrap().unwrap();
        let out2 = res2.unwrap().unwrap();

        let adv =
            usize::from(out1.advanced_round.is_some()) + usize::from(out2.advanced_round.is_some());
        assert_eq!(
            adv, 1,
            "iteration {i} failed: expected exactly 1 round advance"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// Test 6: Scripted API-level run of a project: Draft -> Queued -> P1 -> P2
// ───────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_scripted_project_lifecycle_run() {
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping DB test: database unavailable");
        return;
    };

    let rounds_config = RoundsConfig::default_embedded();
    let mgr = create_test_manager(&pool, "mgr_full").await;
    let proofer = create_test_user(&pool, "prf_full").await;

    // 1. Create project in Draft
    let pid = create_project(
        &pool,
        &dp_db::queries::projects::NewProject {
            name: "Dự án Vòng đời Đầy đủ",
            author: "Nguyễn Du",
            tone_style: "new",
            charset: "vi",
            manager_id: mgr,
            image_source: "Thư viện Quốc gia",
            comments: "Thử nghiệm từ Draft tới P2",
        },
    )
    .await
    .unwrap();

    // 2. Add 2 pages
    let new_pages = vec![
        NewPageInput {
            page_number: 1,
            extension: "png".to_string(),
            image_width: 800,
            image_height: 1200,
            ocr_text: Some("Trăm năm trong cõi người ta".to_string()),
        },
        NewPageInput {
            page_number: 2,
            extension: "png".to_string(),
            image_width: 800,
            image_height: 1200,
            ocr_text: Some("Chữ tài chữ mệnh khéo là ghét nhau".to_string()),
        },
    ];
    let inserted = insert_pages_transactional(&pool, pid, &new_pages)
        .await
        .unwrap();
    assert_eq!(inserted.len(), 2);

    // 3. Manager transitions Draft -> Queued
    let eff1 = transition_project(
        &pool,
        pid,
        Phase::Queued,
        ActorRole::Manager,
        Some(UserId::new(mgr)),
        &rounds_config,
    )
    .await
    .unwrap();
    assert_eq!(eff1.len(), 2);

    let proj = get_project_by_id(&pool, pid).await.unwrap().unwrap();
    assert_eq!(proj.phase, "queued");

    // 4. Manager transitions Queued -> InRound (starts at P1)
    let eff2 = transition_project(
        &pool,
        pid,
        Phase::InRound,
        ActorRole::Manager,
        Some(UserId::new(mgr)),
        &rounds_config,
    )
    .await
    .unwrap();
    assert_eq!(eff2.len(), 2);

    let proj = get_project_by_id(&pool, pid).await.unwrap().unwrap();
    assert_eq!(proj.phase, "in_round");
    assert_eq!(proj.current_round.as_deref(), Some("P1"));

    // 5. Proofer claims page 1 in P1
    let claim1 = claim_next_available_page(&pool, pid, "P1", None, proofer, 1)
        .await
        .unwrap()
        .expect("should claim page 1");
    assert_eq!(claim1.page_number, 1);

    // 6. Proofer saves page 1 in P1 (not the last page yet)
    let outcome1 = save_page(
        &pool,
        SavePageInput {
            project_id: pid,
            page_id: claim1.page_id,
            round: "P1",
            user_id: proofer,
            text: "Trăm năm trong cõi người ta (đã sửa)",
        },
        &rounds_config,
    )
    .await
    .unwrap();
    assert!(outcome1.advanced_round.is_none());

    // 7. Proofer claims page 2 in P1
    let claim2 = claim_next_available_page(&pool, pid, "P1", None, proofer, 1)
        .await
        .unwrap()
        .expect("should claim page 2");
    assert_eq!(claim2.page_number, 2);

    // 8. Proofer saves page 2 in P1 (last page -> advances to P2)
    let outcome2 = save_page(
        &pool,
        SavePageInput {
            project_id: pid,
            page_id: claim2.page_id,
            round: "P1",
            user_id: proofer,
            text: "Chữ tài chữ mệnh khéo là ghét nhau (đã sửa)",
        },
        &rounds_config,
    )
    .await
    .unwrap();

    assert_eq!(outcome2.advanced_round.as_deref(), Some("P2"));

    // Verify project is now in P2
    let proj_p2 = get_project_by_id(&pool, pid).await.unwrap().unwrap();
    assert_eq!(proj_p2.phase, "in_round");
    assert_eq!(proj_p2.current_round.as_deref(), Some("P2"));

    // Verify audit log has recorded the transitions
    let events = list_project_events(&pool, pid, 10, 0).await.unwrap();
    assert!(events.len() >= 3, "audit events must be recorded");

    // Check daily limit enforcement
    let limit_check = check_daily_limit(&pool, proofer, 100).await;
    assert!(limit_check.is_ok());

    let over_limit = check_daily_limit(&pool, proofer, 2).await;
    assert!(
        matches!(
            over_limit,
            Err(dp_db::DbError::Domain(
                dp_core::DomainError::DailyLimitReached { .. }
            ))
        ),
        "user who proofed 2 pages must hit daily limit when limit = 2"
    );
}
