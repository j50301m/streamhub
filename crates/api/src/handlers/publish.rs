use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::stream;
use sea_orm::Set;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PublishHookPayload {
    pub stream_key: String,
    pub action: String,
}

/// POST /internal/hooks/publish
/// Called by MediaMTX on publish/unpublish events.
pub(crate) async fn publish_hook(
    State(state): State<AppState>,
    Json(payload): Json<PublishHookPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(
        stream_key = %payload.stream_key,
        action = %payload.action,
        "Received publish hook"
    );

    let txn = state.uow.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stream = txn
        .stream_repo()
        .find_by_key_for_update(&payload.stream_key)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let stream = match stream {
        Some(s) => s,
        None => {
            tracing::warn!(stream_key = %payload.stream_key, "Stream not found for hook");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let mut active: stream::ActiveModel = stream.into();

    match payload.action.as_str() {
        "publish" => {
            active.status = Set(stream::StreamStatus::Live);
            active.started_at = Set(Some(Utc::now()));
        }
        "unpublish" => {
            active.status = Set(stream::StreamStatus::Ended);
            active.ended_at = Set(Some(Utc::now()));
        }
        _ => {
            tracing::warn!(action = %payload.action, "Unknown hook action");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    txn.stream_repo().update(active).await.map_err(|e| {
        tracing::error!("Failed to update stream: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}
