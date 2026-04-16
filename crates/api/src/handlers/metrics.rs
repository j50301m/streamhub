use crate::state::AppState;
use axum::extract::State;

/// `GET /metrics` — Prometheus text-format exposition.
///
/// Scraped by Prometheus / `node_exporter`-style collectors. Always returns 200.
pub async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}
