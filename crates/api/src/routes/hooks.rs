use axum::Router;
use axum::routing::post;
use streamhub_common::AppState;
use streamhub_hook::publish_hook;

pub fn hook_routes() -> Router<AppState> {
    Router::new().route("/internal/hooks/publish", post(publish_hook))
}
