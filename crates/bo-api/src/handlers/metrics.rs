//! Prometheus scrape endpoint.

use axum::extract::State;

use crate::state::BoAppState;

/// `GET /metrics` — Prometheus text-format exposition.
///
/// Scraped by Prometheus without authentication. Always returns 200.
pub async fn metrics_handler(State(state): State<BoAppState>) -> String {
    state.metrics.render()
}
