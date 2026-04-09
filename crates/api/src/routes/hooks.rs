use axum::Router;
use axum::routing::post;
use common::AppState;
use hook::publish_hook;

pub fn hook_routes() -> Router<AppState> {
    Router::new().route("/internal/hooks/publish", post(publish_hook))
}
