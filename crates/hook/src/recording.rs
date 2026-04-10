use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::{recording, stream};
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
pub async fn recording_hook(
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

        tokio::spawn(async move {
            if let Err(e) = run_transcode(uow, &recordings_path, stream_id, &stream_key).await {
                tracing::error!(stream_id = %stream_id, error = %e, "Transcode task failed");
            }
        });
    }

    Ok(StatusCode::OK)
}

/// Find the latest MP4 recording for a stream, transcode it to HLS,
/// and update the stream's vod_status + hls_url.
async fn run_transcode(
    uow: repo::UnitOfWork,
    recordings_path: &str,
    stream_id: Uuid,
    stream_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let latest_recording = uow
        .recording_repo()
        .find_latest_by_stream(stream_id)
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
            uow.stream_repo().update(active).await?;
            tracing::info!(stream_id = %stream_id, %hls_url, "VOD transcode completed");
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use common::{AppConfig, AppState};
    use repo::UnitOfWork;
    use sea_orm::{DbBackend, MockDatabase, MockExecResult};
    use tower::ServiceExt;

    fn test_config() -> AppConfig {
        AppConfig {
            database_url: String::new(),
            host: "127.0.0.1".to_string(),
            port: 0,
            mediamtx_url: "http://localhost:9997".to_string(),
            jwt_secret: "test-secret".to_string(),
            recordings_path: "/tmp/recordings".to_string(),
        }
    }

    fn ended_stream() -> stream::Model {
        let id = Uuid::new_v4();
        stream::Model {
            id,
            user_id: Some(Uuid::new_v4()),
            stream_key: id.to_string(),
            title: Some("Test".to_string()),
            status: stream::StreamStatus::Ended,
            vod_status: stream::VodStatus::None,
            started_at: Some(Utc::now()),
            ended_at: Some(Utc::now()),
            created_at: Utc::now(),
            hls_url: None,
        }
    }

    fn live_stream() -> stream::Model {
        let id = Uuid::new_v4();
        stream::Model {
            id,
            user_id: Some(Uuid::new_v4()),
            stream_key: id.to_string(),
            title: Some("Test".to_string()),
            status: stream::StreamStatus::Live,
            vod_status: stream::VodStatus::None,
            started_at: Some(Utc::now()),
            ended_at: None,
            created_at: Utc::now(),
            hls_url: None,
        }
    }

    #[tokio::test]
    async fn recording_hook_ended_stream_triggers_transcode() {
        let s = ended_stream();
        let rec = recording::Model {
            id: Uuid::new_v4(),
            stream_id: s.id,
            file_path: "/recordings/test.mp4".to_string(),
            duration_secs: None,
            file_size_bytes: None,
            created_at: Utc::now(),
        };
        let mut updated = s.clone();
        updated.vod_status = stream::VodStatus::Processing;

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]]) // find_by_key_for_update
            .append_query_results([vec![rec.clone()]]) // create recording
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .append_query_results([vec![updated.clone()]]) // update vod_status
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // For the background transcode task (find_latest_recording_by_stream)
            // The task will fail because no real ffmpeg, but the hook itself should return OK
            .append_query_results([vec![rec.clone()]])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = Router::new()
            .route("/internal/hooks/recording", post(recording_hook))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/recording")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": s.stream_key,
                    "segment_path": "/recordings/test.mp4"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn recording_hook_live_stream_no_transcode() {
        let s = live_stream();
        let rec = recording::Model {
            id: Uuid::new_v4(),
            stream_id: s.id,
            file_path: "/recordings/test.mp4".to_string(),
            duration_secs: None,
            file_size_bytes: None,
            created_at: Utc::now(),
        };

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]]) // find_by_key_for_update
            .append_query_results([vec![rec.clone()]]) // create recording
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = Router::new()
            .route("/internal/hooks/recording", post(recording_hook))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/recording")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": s.stream_key,
                    "segment_path": "/recordings/test.mp4"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
