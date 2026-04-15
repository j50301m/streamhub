use axum::Json;
use axum::extract::State;
use common::AppState;
use deadpool_redis::redis::cmd;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

/// `GET /v1/health` — deep health probe.
///
/// Pings PostgreSQL and Redis and returns `{"db": "ok|error", "redis": "ok|error"}`.
/// Always returns 200 so Kubernetes liveness probes can distinguish degraded
/// backends from a dead API process; orchestration should read the body.
#[tracing::instrument(skip(state))]
pub async fn health_check(State(state): State<AppState>) -> Json<Value> {
    let db_status = match state.uow.db().execute_unprepared("SELECT 1").await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    let redis_status = match state.redis_pool.get().await {
        Ok(mut conn) => match cmd("PING").query_async::<String>(&mut conn).await {
            Ok(_) => "ok",
            Err(_) => "error",
        },
        Err(_) => "error",
    };

    Json(json!({
        "db": db_status,
        "redis": redis_status,
    }))
}
