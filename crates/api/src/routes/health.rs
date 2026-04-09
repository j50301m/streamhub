use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use streamhub_common::AppState;

async fn healthz() -> StatusCode {
    StatusCode::OK
}

pub fn health_routes() -> Router<AppState> {
    Router::new().route("/healthz", get(healthz))
}
