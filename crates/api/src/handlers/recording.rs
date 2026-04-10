use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::{AppState, config::AppConfig};
use entity::{recording, stream};
use sea_orm::Set;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage::ObjectStorage;
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
        .find_by_key_for_update(&payload.stream_key)
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

    // If the stream already ended, trigger transcoding
    let should_transcode = stream.status == stream::StreamStatus::Ended;
    let stream_id = stream.id;
    let stream_key = stream.stream_key.clone();

    if should_transcode {
        tracing::info!(stream_id = %stream.id, "Stream ended, triggering transcode");

        // Set vod_status to Processing
        let mut active: stream::ActiveModel = stream.into();
        active.vod_status = Set(stream::VodStatus::Processing);
        txn.stream_repo().update(active).await.map_err(|e| {
            tracing::error!("Failed to update vod_status: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // After commit, spawn background transcode task
    if should_transcode {
        let uow = state.uow.clone();
        let recordings_path = state.config.recordings_path.clone();
        let storage = state.storage.clone();
        let config = state.config.clone();

        tokio::spawn(async move {
            if let Err(e) = run_transcode(
                uow,
                &recordings_path,
                stream_id,
                &stream_key,
                storage,
                &config,
            )
            .await
            {
                tracing::error!(stream_id = %stream_id, error = %e, "Transcode task failed");
            }
        });
    }

    Ok(StatusCode::OK)
}

/// Find the latest MP4 recording for a stream, transcode it to HLS,
/// optionally upload to GCS, and update the stream's vod_status + hls_url.
///
/// When `config.transcoder_enabled()` is true, uploads the raw MP4 to GCS
/// and creates a GCP Transcoder API job (async — Pub/Sub webhook updates status later).
/// Otherwise, uses local ffmpeg transcoding.
async fn run_transcode(
    uow: repo::UnitOfWork,
    recordings_path: &str,
    stream_id: Uuid,
    stream_key: &str,
    storage: Option<Arc<dyn ObjectStorage>>,
    config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let latest_recording = uow
        .recording_repo()
        .find_latest_by_stream(stream_id)
        .await?
        .ok_or("no recording found")?;

    let input_mp4 = map_to_local_path(&latest_recording.file_path, recordings_path);

    if config.transcoder_enabled() {
        run_transcode_gcp(&uow, &input_mp4, stream_id, stream_key, &storage, config).await
    } else {
        run_transcode_local(
            &uow,
            &input_mp4,
            recordings_path,
            stream_id,
            stream_key,
            storage,
        )
        .await
    }
}

/// GCP Transcoder API path: upload MP4 to GCS, then create a transcoder job.
/// The job runs asynchronously — a Pub/Sub webhook will update vod_status when done.
async fn run_transcode_gcp(
    uow: &repo::UnitOfWork,
    input_mp4: &Path,
    stream_id: Uuid,
    stream_key: &str,
    storage: &Option<Arc<dyn ObjectStorage>>,
    config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let store = storage
        .as_ref()
        .ok_or("GCS storage is required when TRANSCODER_ENABLED=true")?;

    // Upload raw MP4 to GCS
    let mp4_key = format!("streams/{}/input.mp4", stream_key);
    store
        .upload_file(input_mp4, &mp4_key)
        .await
        .map_err(|e| format!("GCS upload failed: {e}"))?;
    tracing::info!(stream_id = %stream_id, %mp4_key, "Uploaded raw MP4 to GCS");

    // Get auth token and create transcoder job
    let token = transcoder::get_gcp_token().await?;
    let input_uri = format!("gs://{}/{}", config.gcs_bucket, mp4_key);
    let output_uri = format!("gs://{}/streams/{}/output/", config.gcs_bucket, stream_key);

    let job_name = transcoder::create_job(
        &config.transcoder_project_id,
        &config.transcoder_location,
        &input_uri,
        &output_uri,
        &stream_id.to_string(),
        &token,
    )
    .await?;

    tracing::info!(stream_id = %stream_id, %job_name, "Transcoder job created, waiting for Pub/Sub callback");

    // Set hls_url to the expected output path (will be confirmed by webhook)
    let hls_url = format!(
        "https://storage.googleapis.com/{}/streams/{}/output/index.m3u8",
        config.gcs_bucket, stream_key
    );
    let active = stream::ActiveModel {
        id: Set(stream_id),
        hls_url: Set(Some(hls_url)),
        ..Default::default()
    };
    uow.stream_repo().update(active).await?;

    Ok(())
}

/// Local ffmpeg path: transcode to HLS, optionally upload to GCS, update vod_status=Ready.
async fn run_transcode_local(
    uow: &repo::UnitOfWork,
    input_mp4: &Path,
    recordings_path: &str,
    stream_id: Uuid,
    stream_key: &str,
    storage: Option<Arc<dyn ObjectStorage>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let output_dir = PathBuf::from(recordings_path).join(stream_key).join("hls");

    tracing::info!(
        stream_id = %stream_id,
        input = %input_mp4.display(),
        output_dir = %output_dir.display(),
        "Starting local VOD transcode"
    );

    match transcoder::transcode_to_hls(input_mp4, &output_dir).await {
        Ok(_) => {
            let hls_url = if let Some(ref store) = storage {
                let gcs_prefix = format!("streams/{}/hls", stream_key);
                store
                    .upload_dir(&output_dir, &gcs_prefix)
                    .await
                    .map_err(|e| format!("GCS upload failed: {e}"))?;

                let url = store.public_url(&format!("{}/index.m3u8", gcs_prefix));
                tracing::info!(stream_id = %stream_id, %url, "HLS uploaded to GCS");
                url
            } else {
                format!("/vod/{stream_key}/hls/index.m3u8")
            };

            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Ready),
                hls_url: Set(Some(hls_url.clone())),
                ..Default::default()
            };
            uow.stream_repo().update(active).await?;
            tracing::info!(stream_id = %stream_id, %hls_url, "VOD transcode completed");

            // Clean up local HLS files after successful GCS upload
            if storage.is_some() {
                if let Err(e) = tokio::fs::remove_dir_all(&output_dir).await {
                    tracing::warn!(
                        path = %output_dir.display(),
                        error = %e,
                        "Failed to clean up local HLS directory"
                    );
                } else {
                    tracing::info!(path = %output_dir.display(), "Cleaned up local HLS directory");
                }
            }
        }
        Err(e) => {
            tracing::error!(stream_id = %stream_id, error = %e, "VOD transcode failed");
            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Failed),
                ..Default::default()
            };
            uow.stream_repo().update(active).await?;
        }
    }

    Ok(())
}
