use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use streamhub_common::AppState;
use streamhub_entity::stream;

#[derive(Debug, Deserialize)]
pub struct PublishHookPayload {
    pub stream_key: String,
    pub action: String,
}

/// POST /internal/hooks/publish
/// Called by MediaMTX on publish/unpublish events.
pub async fn publish_hook(
    State(state): State<AppState>,
    Json(payload): Json<PublishHookPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(
        stream_key = %payload.stream_key,
        action = %payload.action,
        "Received publish hook"
    );

    let stream = stream::Entity::find()
        .filter(stream::Column::StreamKey.eq(&payload.stream_key))
        .one(&state.db)
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

    active.update(&state.db).await.map_err(|e| {
        tracing::error!("Failed to update stream: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}
