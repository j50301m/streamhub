use axum::extract::{Query, State};
use axum::http::StatusCode;
use common::AppState;
use mediamtx::keys;
use serde::Deserialize;

use crate::ws::types::RedisEvent;

/// Query string for `POST /internal/mtx/drain`.
#[derive(Debug, Deserialize)]
pub struct DrainQuery {
    /// MediaMTX instance name to drain (e.g. `"mtx-2"`).
    pub mtx: String,
}

/// `POST /internal/mtx/drain?mtx=<name>` — mark an MTX instance as draining and
/// tell affected clients to reconnect so they migrate to a healthy instance.
///
/// Internal; not exposed outside the cluster.
///
/// # Errors
/// - 500 on Redis, DB, or pubsub failure
#[tracing::instrument(skip(state), fields(mtx = %query.mtx))]
pub(crate) async fn drain_handler(
    State(state): State<AppState>,
    Query(query): Query<DrainQuery>,
) -> Result<StatusCode, StatusCode> {
    let mtx_name = &query.mtx;

    state
        .cache
        .set(&keys::mtx_status(mtx_name), "draining", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to set draining status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(mtx = %mtx_name, "Marked MTX instance as draining");

    let live_streams = state.uow.stream_repo().list_live().await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list live streams");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let live_ids: Vec<uuid::Uuid> = live_streams.iter().map(|s| s.id).collect();

    let hits = mediamtx::get_streams_on_mtx(state.cache.as_ref(), &live_ids, mtx_name)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to find streams on MTX");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let affected_stream_ids: Vec<uuid::Uuid> =
        hits.into_iter().map(|(stream_id, _)| stream_id).collect();

    if affected_stream_ids.is_empty() {
        tracing::info!(mtx = %mtx_name, "No active streams on this instance");
        return Ok(StatusCode::OK);
    }

    tracing::info!(
        mtx = %mtx_name,
        count = affected_stream_ids.len(),
        "Publishing reconnect event for affected streams"
    );

    let event = RedisEvent::Reconnect {
        reason: "server_maintenance".to_string(),
        stream_ids: affected_stream_ids,
    };
    let event_json = serde_json::to_string(&event).map_err(|e| {
        tracing::error!(error = %e, "Failed to serialize reconnect event");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    state
        .pubsub
        .publish("streamhub:events", &event_json)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to publish reconnect event");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}
