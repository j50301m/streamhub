use axum::Router;
use axum::routing::post;
use common::AppState;
use hook::{mediamtx_auth, publish_hook, recording_hook};

pub fn hook_routes() -> Router<AppState> {
    Router::new()
        .route("/internal/hooks/publish", post(publish_hook))
        .route("/internal/hooks/recording", post(recording_hook))
        .route("/internal/auth", post(mediamtx_auth))
}
