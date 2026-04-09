use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::{recording, stream};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct RecordingHookPayload {
    pub stream_key: String,
    pub segment_path: String,
}

/// Map a container-internal recording path to the host-local path.
fn map_to_local_path(segment_path: &str, recordings_path: &str) -> PathBuf {
    let relative = segment_path
        .strip_prefix("/recordings/")
        .or_else(|| segment_path.strip_prefix("/recordings"))
        .unwrap_or(segment_path);
    Path::new(recordings_path).join(relative)
}

/// POST /internal/hooks/recording
/// Called by MediaMTX when a recording segment is complete.
pub async fn recording_hook(
    State(state): State<AppState>,
    Json(payload): Json<RecordingHookPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(
        stream_key = %payload.stream_key,
        segment_path = %payload.segment_path,
        "Received recording hook"
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
            tracing::warn!(stream_key = %payload.stream_key, "Stream not found for recording hook");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // Map the container path to the local filesystem path and read file size
    let local_path = map_to_local_path(&payload.segment_path, &state.recordings_path);
    let file_size_bytes = match tokio::fs::metadata(&local_path).await {
        Ok(meta) => Some(meta.len() as i64),
        Err(e) => {
            tracing::warn!(
                path = %local_path.display(),
                error = %e,
                "Could not read recording file metadata, storing without size"
            );
            None
        }
    };

    // Create recording record
    let rec = recording::ActiveModel {
        id: Set(Uuid::new_v4()),
        stream_id: Set(stream.id),
        file_path: Set(payload.segment_path.clone()),
        duration_secs: Set(None),
        file_size_bytes: Set(file_size_bytes),
        created_at: Set(Utc::now()),
    };

    rec.insert(&state.db).await.map_err(|e| {
        tracing::error!("Failed to insert recording: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        stream_id = %stream.id,
        file_size_bytes = ?file_size_bytes,
        "Recording segment saved"
    );

    // If the stream already ended, trigger transcoding
    if stream.status == stream::StreamStatus::Ended {
        tracing::info!(stream_id = %stream.id, "Stream ended, triggering transcode");

        let db = state.db.clone();
        let recordings_path = state.recordings_path.clone();
        let stream_id = stream.id;
        let stream_key = stream.stream_key.clone();

        // Set vod_status to Processing
        let mut active: stream::ActiveModel = stream.into();
        active.vod_status = Set(stream::VodStatus::Processing);
        active.update(&state.db).await.map_err(|e| {
            tracing::error!("Failed to update vod_status: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

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
    stream_id: Uuid,
    stream_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
