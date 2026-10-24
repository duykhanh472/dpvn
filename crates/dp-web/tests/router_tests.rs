use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use dp_web::{AppConfig, AppState, AssetManifest, create_router, create_template_env};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_health_routes_return_service_unavailable_without_db() {
    // When DB is unreachable, healthz must return 503 rather than panic
    let config = Arc::new(AppConfig::default());
    let manifest = Arc::new(AssetManifest::default());
    let templates = Arc::new(create_template_env(&config, manifest.clone()).unwrap());

    // Create an un-connected pool using a non-existent port with immediate timeout
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("mysql://dp:secret@127.0.0.1:54321/dp")
        .expect("pool lazy creation failed");

    let state = AppState {
        db: pool,
        config,
        templates,
        manifest,
        rate_limiter: Arc::new(dp_web::RateLimiter::new()),
    };

    let app = create_router(state);

    // 1. Test /healthz returns 503 Service Unavailable when DB is down
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // 2. Test /readyz returns 503 Service Unavailable when DB is down
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // 3. Test home page renders 200 OK
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // Request ID header must be attached
    assert!(response.headers().contains_key("x-request-id"));
}
