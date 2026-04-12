mod auth_test;
mod hooks_test;
mod streams_test;

use axum::body::Body;
use common::AppConfig;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusHandle;

const JWT_SECRET: &str = "test-secret";

pub(crate) fn test_metrics() -> PrometheusHandle {
    // Each test needs its own recorder; use a throwaway builder.
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle()
}

pub(crate) fn test_config() -> AppConfig {
    AppConfig {
        database_url: String::new(),
        host: "127.0.0.1".to_string(),
        port: 0,
        mediamtx_url: "http://localhost:9997".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        recordings_path: "/tmp/recordings".to_string(),
        thumbnails_path: "/tmp/thumbnails".to_string(),
        storage_enabled: "false".to_string(),
        gcs_bucket: "test-bucket".to_string(),
        gcs_endpoint: String::new(),
        gcs_credentials_path: String::new(),
        transcoder_enabled: "false".to_string(),
        transcoder_project_id: String::new(),
        transcoder_location: "asia-east1".to_string(),
        pubsub_verify_token: String::new(),
        otel_endpoint: "http://localhost:4317".to_string(),
    }
}

async fn body_to_json(body: Body) -> serde_json::Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
