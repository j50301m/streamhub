use axum::Router;
use axum::routing::{get, post};
use serde_json::{Value, json};

use crate::handlers;
use crate::state::BoAppState;

async fn healthz() -> axum::Json<Value> {
    axum::Json(json!({"status": "ok"}))
}

pub fn app_router() -> Router<BoAppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/auth/login", post(handlers::auth::login))
        .route("/v1/auth/refresh", post(handlers::auth::refresh))
        .route("/v1/admin/dashboard", get(handlers::dashboard::dashboard))
}
