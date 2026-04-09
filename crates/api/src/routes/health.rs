use axum::Json;
use axum::Router;
use axum::routing::get;
use common::AppState;
use serde_json::{Value, json};

async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

pub fn health_routes() -> Router<AppState> {
    Router::new().route("/healthz", get(healthz))
}
