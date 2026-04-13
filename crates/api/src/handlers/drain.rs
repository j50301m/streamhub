use axum::extract::{Query, State};
use axum::http::StatusCode;
use common::AppState;
use serde::Deserialize;

use crate::ws::types::RedisEvent;

#[derive(Debug, Deserialize)]
pub struct DrainQuery {
    /// MediaMTX instance name to drain (e.g. "mtx-2")
    pub mtx: String,
}

/// POST /internal/mtx/drain?mtx=mtx-2
/// Marks an MTX instance as draining and notifies affected clients to reconnect.
#[tracing::instrument(skip(state), fields(mtx = %query.mtx))]
pub(crate) async fn drain_handler(
    State(state): State<AppState>,
    Query(query): Query<DrainQuery>,
) -> Result<StatusCode, StatusCode> {
    let mtx_name = &query.mtx;

    // Mark as draining in Redis (unified status key, no TTL)
    state
        .cache
        .set(&format!("mtx:{mtx_name}:status"), "draining", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to set draining status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(mtx = %mtx_name, "Marked MTX instance as draining");

    // Find all live streams on this MTX instance
    let live_streams = state.uow.stream_repo().list_live().await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list live streams");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut affected_stream_ids = Vec::new();
    for s in &live_streams {
        let stream_id_str = s.id.to_string();
        let mapped_mtx = state
            .cache
            .get(&format!("stream:{stream_id_str}:mtx"))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get stream MTX mapping");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        if mapped_mtx.as_deref() == Some(mtx_name) {
            affected_stream_ids.push(s.id);
        }
    }

    if affected_stream_ids.is_empty() {
        tracing::info!(mtx = %mtx_name, "No active streams on this instance");
        return Ok(StatusCode::OK);
    }

    tracing::info!(
        mtx = %mtx_name,
        count = affected_stream_ids.len(),
        "Publishing reconnect event for affected streams"
    );

    // Publish reconnect event via Redis PubSub
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
