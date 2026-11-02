//! Phase 11 hardening regression tests.
//!
//! These tests are intentionally cheap: they assert the security headers and
//! Vietnamese error pages at the HTTP layer, and check the deployment files for
//! the invariants that the launch checklist depends on.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use dp_web::{AppConfig, AppState, AssetManifest, create_router, create_template_env};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

fn test_app() -> axum::Router {
    let config = Arc::new(AppConfig::default());
    let manifest = Arc::new(AssetManifest::default());
    let templates = Arc::new(create_template_env(&config, manifest.clone()).unwrap());
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("mysql://dp:secret@127.0.0.1:54321/dp")
        .expect("pool lazy creation failed");

    create_router(AppState {
        db: pool,
        config,
        templates,
        manifest,
        rate_limiter: Arc::new(dp_web::RateLimiter::new()),
        site_words: Arc::new(tokio::sync::RwLock::new(Default::default())),
    })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[tokio::test]
async fn unknown_route_returns_vietnamese_404_with_security_headers() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/this-page-does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("CSP header missing");
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
    assert!(!csp.contains("unsafe-eval"));

    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains(r#"lang="vi""#));
    assert!(html.contains("Không tìm thấy trang"));
    assert!(html.contains("Quay lại trang chủ"));
}

#[tokio::test]
async fn every_response_gets_nosniff_and_referrer_policy() {
    let response = test_app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        response
            .headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        response
            .headers()
            .get(header::REFERRER_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some("same-origin")
    );
    assert_eq!(
        response
            .headers()
            .get(header::X_FRAME_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("DENY")
    );
}

#[test]
fn templates_do_not_use_inline_event_handlers_or_hx_on() {
    let templates = repo_root().join("crates/dp-web/src/templates");
    let mut files = Vec::new();
    collect_html(&templates, &mut files);
    assert!(!files.is_empty(), "no templates found");

    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        let name = file.display();
        assert!(
            !source.contains("onsubmit="),
            "{name} uses an inline onsubmit handler (blocked by CSP)"
        );
        assert!(
            !source.contains("onclick="),
            "{name} uses an inline onclick handler (blocked by CSP)"
        );
        assert!(
            !source.contains("hx-on"),
            "{name} uses hx-on, which requires unsafe-eval"
        );
    }
}

#[test]
fn base_template_has_skip_link_and_focusable_main() {
    let base = fs::read_to_string(repo_root().join("crates/dp-web/src/templates/base.html"))
        .expect("base template");
    assert!(base.contains("class=\"skip-link\""));
    assert!(base.contains("id=\"main-content\""));
    assert!(base.contains("aria-live=\"polite\""));
}

#[test]
fn proof_template_exposes_keyboard_and_live_state() {
    let proof =
        fs::read_to_string(repo_root().join("crates/dp-web/src/templates/proof/proof.html"))
            .expect("proof template");
    assert!(proof.contains("aria-keyshortcuts"));
    assert!(proof.contains("aria-valuenow=\"50\""));
    assert!(proof.contains("role=\"status\""));
}

#[test]
fn deployment_files_encode_the_hardening_invariants() {
    let root = repo_root();

    let dockerfile = fs::read_to_string(root.join("Dockerfile")).expect("Dockerfile");
    assert!(dockerfile.contains("gcr.io/distroless/cc-debian12:nonroot"));
    assert!(dockerfile.contains("USER 65532:65532"));
    assert!(dockerfile.contains("HEALTHCHECK"));
    assert!(dockerfile.contains("dp\", \"healthcheck"));

    let headers =
        fs::read_to_string(root.join("deploy/security-headers.inc")).expect("security headers");
    assert!(headers.contains("script-src 'self'"));
    let csp_line = headers
        .lines()
        .find(|line| line.contains("Content-Security-Policy"))
        .expect("CSP directive");
    assert!(!csp_line.contains("unsafe-eval"));
    assert!(!csp_line.contains("script-src 'self' 'unsafe-inline'"));

    let nginx = fs::read_to_string(root.join("deploy/nginx.conf")).expect("nginx.conf");
    assert!(nginx.contains("ssl_protocols"));
    assert!(nginx.contains("gzip on"));
    assert!(nginx.contains("location /protected/"));
    assert!(nginx.contains("internal;"));

    for path in [
        "deploy/backup.sh",
        "deploy/restore.sh",
        "deploy/restore-drill.sh",
        "scripts/loadtest.js",
        "docs/operations/deploy.md",
        "docs/operations/runbook.md",
        "docs/operations/launch-checklist.md",
        "docs/operations/restore-drill-log.md",
    ] {
        assert!(root.join(path).exists(), "missing {path}");
    }
}

fn collect_html(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read template dir") {
        let entry = entry.expect("template entry");
        let path = entry.path();
        if path.is_dir() {
            collect_html(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "html") {
            out.push(path);
        }
    }
}
