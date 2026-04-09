use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::{recording, stream};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use std::path::PathBuf;

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

    let stream_id = stream.id;
    let stream_key = stream.stream_key.clone();
    let mut active: stream::ActiveModel = stream.into();

    match payload.action.as_str() {
        "publish" => {
            active.status = Set(stream::StreamStatus::Live);
            active.started_at = Set(Some(Utc::now()));
        }
        "unpublish" => {
            active.status = Set(stream::StreamStatus::Ended);
            active.ended_at = Set(Some(Utc::now()));

            // Check if there are any recordings
            let latest_recording = recording::Entity::find()
                .filter(recording::Column::StreamId.eq(stream_id))
                .order_by_desc(recording::Column::CreatedAt)
                .one(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!("Database error checking recordings: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;

            if latest_recording.is_some() {
                active.vod_status = Set(stream::VodStatus::Processing);
            }
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

    // Spawn transcoding task after responding (only for unpublish with recordings)
    if payload.action == "unpublish" {
        let db = state.db.clone();
        let recordings_path = state.recordings_path.clone();

        tokio::spawn(async move {
            if let Err(e) = run_transcode(db, &recordings_path, stream_id, &stream_key).await {
                tracing::error!(stream_id = %stream_id, error = %e, "Transcode task failed");
            }
        });
    }

    Ok(StatusCode::OK)
}

/// Find the latest MP4 recording for a stream, transcode it to HLS,
/// and update the stream's vod_status + hls_url.
async fn run_transcode(
    db: sea_orm::DatabaseConnection,
    recordings_path: &str,
    stream_id: uuid::Uuid,
    stream_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check if vod_status is Processing (meaning there are recordings to transcode)
    let stream_model = stream::Entity::find_by_id(stream_id)
        .one(&db)
        .await?
        .ok_or("stream not found")?;

    if stream_model.vod_status != stream::VodStatus::Processing {
        tracing::info!(stream_id = %stream_id, "No recordings to transcode, skipping");
        return Ok(());
    }

    // Find the latest recording file
    let latest_recording = recording::Entity::find()
        .filter(recording::Column::StreamId.eq(stream_id))
        .order_by_desc(recording::Column::CreatedAt)
        .one(&db)
        .await?
        .ok_or("no recording found")?;

    let input_mp4 = PathBuf::from(&latest_recording.file_path);
    let output_dir = PathBuf::from(recordings_path).join(stream_key).join("hls");

    tracing::info!(
        stream_id = %stream_id,
        input = %input_mp4.display(),
        output_dir = %output_dir.display(),
        "Starting VOD transcode"
    );

    match transcoder::transcode_to_hls(&input_mp4, &output_dir).await {
        Ok(_) => {
            let hls_url = format!("/vod/{stream_key}/hls/index.m3u8");
            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Ready),
                hls_url: Set(Some(hls_url.clone())),
                ..Default::default()
            };
            active.update(&db).await?;
            tracing::info!(stream_id = %stream_id, %hls_url, "VOD transcode completed");
        }
        Err(e) => {
            tracing::error!(stream_id = %stream_id, error = %e, "VOD transcode failed");
            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Failed),
                ..Default::default()
            };
            active.update(&db).await?;
        }
    }

    Ok(())
}
