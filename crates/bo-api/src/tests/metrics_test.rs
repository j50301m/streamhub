//! Verifies the unauthed `/metrics` scrape endpoint.
//!
//! The Prometheus recorder is process-global; once installed it cannot be
//! re-installed from another test. These tests therefore avoid
//! `install_recorder()` and instead build a local (non-installed)
//! recorder. The middleware `counter!()` call still compiles and runs but
//! writes to the noop global recorder — the test still verifies:
//!
//! 1. `/metrics` is reachable without JWT / rate-limit auth;
//! 2. the response body renders through the handle we own;
//! 3. when we seed a metric through the same recorder handle, it appears
//!    in the rendered output.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase};
use std::sync::Arc;
use telemetry::metrics_api::{Key, Label, Level, Metadata, Recorder};
use tower::ServiceExt;

use super::test_config;
use crate::routes;
use crate::state::BoAppState;

fn test_state(handle: telemetry::PrometheusHandle) -> BoAppState {
    let db = MockDatabase::new(DbBackend::Postgres).into_connection();
    BoAppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        cache: Arc::new(cache::InMemoryCache::new()),
        pubsub: Arc::new(cache::InMemoryPubSub::new()),
        metrics: handle,
    }
}

fn test_router(state: BoAppState) -> axum::Router {
    axum::Router::new()
        .merge(routes::metrics_router())
        .layer(axum::middleware::from_fn(telemetry::base_http_metrics))
        .with_state(state)
}

#[tokio::test]
async fn metrics_endpoint_returns_200_without_auth() {
    // Build an isolated recorder; we do not install it globally so parallel
    // tests can run without "recorder already installed" errors.
    let recorder = telemetry::PrometheusBuilder::new()
        .set_buckets(&[
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ])
        .unwrap()
        .build_recorder();
    let handle = recorder.handle();
    let router = test_router(test_state(handle));

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_body_exposes_http_request_families() {
    // Seed the recorder directly so the rendered body contains both
    // metric families that the middleware would otherwise emit. We do not
    // rely on the middleware because `metrics::counter!()` targets the
    // global recorder, which is not installed in tests.
    let recorder = telemetry::PrometheusBuilder::new()
        .set_buckets(&[
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ])
        .unwrap()
        .build_recorder();
    let handle = recorder.handle();

    let labels = [
        Label::new("method", "GET"),
        Label::new("path", "/metrics"),
        Label::new("status", "200"),
    ];
    let counter_key = Key::from_parts("http_requests_total", labels.to_vec());
    let histogram_key = Key::from_parts("http_request_duration_seconds", labels.to_vec());

    recorder
        .register_counter(&counter_key, &Metadata::new("test", Level::INFO, None))
        .increment(1);
    recorder
        .register_histogram(&histogram_key, &Metadata::new("test", Level::INFO, None))
        .record(0.012);

    let router = test_router(test_state(handle));

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();

    assert!(
        body.contains("http_requests_total"),
        "rendered /metrics body missing http_requests_total. body:\n{body}"
    );
    assert!(
        body.contains("http_request_duration_seconds"),
        "rendered /metrics body missing http_request_duration_seconds. body:\n{body}"
    );
}
