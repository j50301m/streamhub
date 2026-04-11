use axum::extract::State;
use common::AppState;

pub async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}
