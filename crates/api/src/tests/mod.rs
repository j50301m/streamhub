mod auth_test;
mod hooks_test;
mod streams_test;

use axum::body::Body;
use common::AppConfig;
use http_body_util::BodyExt;

const JWT_SECRET: &str = "test-secret";

fn test_config() -> AppConfig {
    AppConfig {
        database_url: String::new(),
        host: "127.0.0.1".to_string(),
        port: 0,
        mediamtx_url: "http://localhost:9997".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        recordings_path: "/tmp/recordings".to_string(),
    }
}

async fn body_to_json(body: Body) -> serde_json::Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
