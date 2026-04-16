mod admin_test;
mod users_test;

use axum::body::Body;
use http_body_util::BodyExt;

use crate::config::BoConfig;

const JWT_SECRET: &str = "test-secret";

pub(crate) fn test_config() -> BoConfig {
    BoConfig {
        database_url: String::new(),
        redis_url: "redis://localhost:6379".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        host: "127.0.0.1".to_string(),
        port: 8800,
        cors_origins: "http://localhost:3000".to_string(),
    }
}

async fn body_to_json(body: Body) -> serde_json::Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
