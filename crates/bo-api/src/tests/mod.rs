mod admin_test;
mod metrics_test;
mod streams_test;
mod users_test;

use axum::body::Body;
use http_body_util::BodyExt;
use telemetry::PrometheusHandle;

use crate::config::BoConfig;

const JWT_SECRET: &str = "test-secret";

pub(crate) fn test_metrics() -> PrometheusHandle {
    // Each test needs its own isolated recorder so label cardinality from
    // other tests does not leak in. The global recorder is not installed.
    telemetry::PrometheusBuilder::new()
        .build_recorder()
        .handle()
}

pub(crate) fn test_config() -> BoConfig {
    BoConfig {
        database_url: String::new(),
        redis_url: "redis://localhost:6379".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        host: "127.0.0.1".to_string(),
        port: 8800,
        cors_origins: "http://localhost:3000".to_string(),
        otel_endpoint: "http://localhost:4317".to_string(),
        rate_limit_general_limit: 60,
        rate_limit_general_window: 60,
    }
}

async fn body_to_json(body: Body) -> serde_json::Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
