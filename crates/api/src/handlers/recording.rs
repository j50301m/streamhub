use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::recording;
use sea_orm::Set;
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
/// Only saves the recording metadata to DB. Transcoding is triggered by unpublish hook.
pub(crate) async fn recording_hook(
    State(state): State<AppState>,
    Json(payload): Json<RecordingHookPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(
        stream_key = %payload.stream_key,
        segment_path = %payload.segment_path,
        "Received recording hook"
    );

    let txn = state.uow.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stream = txn
        .stream_repo()
        .find_by_key(&payload.stream_key)
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
    let local_path = map_to_local_path(&payload.segment_path, &state.config.recordings_path);
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

    txn.recording_repo().create(rec).await.map_err(|e| {
        tracing::error!("Failed to insert recording: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        stream_id = %stream.id,
        file_size_bytes = ?file_size_bytes,
        "Recording segment saved"
    );

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}
