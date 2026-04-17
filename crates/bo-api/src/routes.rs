use axum::Router;
use axum::routing::{delete, get, patch, post};
use serde_json::{Value, json};

use crate::handlers;
use crate::state::BoAppState;

async fn healthz() -> axum::Json<Value> {
    axum::Json(json!({"status": "ok"}))
}

/// Unauthed scrape sub-router — mounted outside JWT / rate-limit layers so
/// Prometheus can poll without credentials.
pub fn metrics_router() -> Router<BoAppState> {
    Router::new().route("/metrics", get(handlers::metrics::metrics_handler))
}

pub fn app_router() -> Router<BoAppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/auth/login", post(handlers::auth::login))
        .route("/v1/auth/refresh", post(handlers::auth::refresh))
        .route("/v1/admin/dashboard", get(handlers::dashboard::dashboard))
        .route("/v1/admin/users", get(handlers::users::list_users))
        .route(
            "/v1/admin/users/{id}/role",
            patch(handlers::users::update_role),
        )
        .route(
            "/v1/admin/users/{id}/suspend",
            post(handlers::users::suspend),
        )
        .route(
            "/v1/admin/users/{id}/suspend",
            delete(handlers::users::unsuspend),
        )
        .route("/v1/admin/streams", get(handlers::streams::list_streams))
        .route(
            "/v1/admin/streams/{id}",
            get(handlers::streams::stream_detail),
        )
        .route(
            "/v1/admin/streams/{id}/end",
            post(handlers::streams::force_end),
        )
        .route(
            "/v1/admin/moderation/bans",
            get(handlers::moderation::list_bans),
        )
        .route(
            "/v1/admin/moderation/streams/{id}/chat",
            get(handlers::moderation::stream_chat),
        )
}
