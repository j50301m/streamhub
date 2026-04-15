use axum::extract::State;
use common::AppState;

/// `GET /metrics` — Prometheus text-format exposition.
///
/// Scraped by Prometheus / `node_exporter`-style collectors. Always returns 200.
pub async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}
