use axum::Router;
use axum::routing::{get, post};
use common::AppState;
use serde_json::{Value, json};

use crate::handlers;

async fn healthz() -> axum::Json<Value> {
    axum::Json(json!({"status": "ok"}))
}

pub fn app_router() -> Router<AppState> {
    Router::new()
        // Health
        .route("/healthz", get(healthz))
        // Auth
        .route("/v1/auth/register", post(handlers::auth::register))
        .route("/v1/auth/login", post(handlers::auth::login))
        .route("/v1/auth/refresh", post(handlers::auth::refresh))
        .route("/v1/auth/logout", post(handlers::auth::logout))
        .route("/v1/me", get(handlers::auth::me))
        // Streams
        .route(
            "/v1/streams",
            post(handlers::streams::create_stream).get(handlers::streams::list_streams),
        )
        .route(
            "/v1/streams/live",
            get(handlers::streams::list_live_streams),
        )
        .route("/v1/streams/vod", get(handlers::streams::list_vod_streams))
        .route(
            "/v1/streams/{id}",
            get(handlers::streams::get_stream)
                .patch(handlers::streams::update_stream)
                .delete(handlers::streams::delete_stream),
        )
        .route("/v1/streams/{id}/end", post(handlers::streams::end_stream))
        .route(
            "/v1/streams/{id}/token",
            post(handlers::streams::create_stream_token),
        )
        .route(
            "/v1/streams/{id}/recordings",
            get(handlers::streams::list_recordings),
        )
        // Internal hooks
        .route(
            "/internal/hooks/publish",
            post(handlers::publish::publish_hook),
        )
        .route(
            "/internal/hooks/recording",
            post(handlers::recording::recording_hook),
        )
        .route(
            "/internal/auth",
            post(handlers::mediamtx_auth::mediamtx_auth),
        )
}
