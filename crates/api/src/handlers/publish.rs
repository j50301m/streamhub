use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::stream;
use sea_orm::Set;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use storage::ObjectStorage;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PublishHookPayload {
    pub stream_key: String,
    pub action: String,
}

/// POST /internal/hooks/publish
/// Called by MediaMTX on publish/unpublish events.
#[tracing::instrument(skip(state, payload), fields(stream_key = %payload.stream_key, action = %payload.action))]
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

    let stream_id = stream.id;
    let stream_key = stream.stream_key.clone();
    let mut active: stream::ActiveModel = stream.into();

    let should_transcode = match payload.action.as_str() {
        "publish" => {
            active.status = Set(stream::StreamStatus::Live);
            active.started_at = Set(Some(Utc::now()));
            false
        }
        "unpublish" => {
            active.status = Set(stream::StreamStatus::Ended);
            active.ended_at = Set(Some(Utc::now()));
            active.vod_status = Set(stream::VodStatus::Processing);
            true
        }
        _ => {
            tracing::warn!(action = %payload.action, "Unknown hook action");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    txn.stream_repo().update(active).await.map_err(|e| {
        tracing::error!("Failed to update stream: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Manage periodic thumbnail capture task
    match payload.action.as_str() {
        "publish" => {
            spawn_thumbnail_task(&state, stream_id, &stream_key).await;
        }
        "unpublish" => {
            cancel_thumbnail_task(&state, stream_id).await;
        }
        _ => {}
    }

    // After unpublish: scan filesystem for recordings and trigger transcode
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

/// Spawn a periodic task that captures a thumbnail from the live HLS stream every 60s.
async fn spawn_thumbnail_task(state: &AppState, stream_id: Uuid, stream_key: &str) {
    let token = CancellationToken::new();
    {
        let mut tasks = state.live_tasks.lock().await;
        tasks.insert(stream_id, token.clone());
    }

    let uow = state.uow.clone();
    let storage = state.storage.clone();
    let thumbnails_path = state.config.thumbnails_path.clone();
    let capture_interval = state.config.thumbnail_capture_interval_secs;
    let stream_key = stream_key.to_string();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(capture_interval));
        // Skip the first immediate tick
        interval.tick().await;

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::info!(stream_id = %stream_id, "Thumbnail capture task cancelled");
                    break;
                }
                _ = interval.tick() => {
                    let hls_url = format!("http://mediamtx:8888/{}/index.m3u8", stream_key);
                    let thumb_dir = PathBuf::from(&thumbnails_path).join(&stream_key);
                    let thumb_path = thumb_dir.join("live-thumb.jpg");

                    match transcoder::capture_hls_thumbnail(&hls_url, &thumb_path).await {
                        Ok(_) => {
                            let thumbnail_url = if let Some(store) = &storage {
                                let key = format!("streams/{}/live-thumb.jpg", stream_key);
                                match store.upload_file(&thumb_path, &key).await {
                                    Ok(_) => store.public_url(&key),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Failed to upload thumbnail to storage");
                                        continue;
                                    }
                                }
                            } else {
                                format!("/thumbnails/{}/live-thumb.jpg", stream_key)
                            };

                            let active = stream::ActiveModel {
                                id: Set(stream_id),
                                thumbnail_url: Set(Some(thumbnail_url)),
                                ..Default::default()
                            };
                            if let Err(e) = uow.stream_repo().update(active).await {
                                tracing::warn!(error = %e, "Failed to update thumbnail_url in DB");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(stream_id = %stream_id, error = %e, "HLS thumbnail capture failed");
                        }
                    }
                }
            }
        }
    });

    tracing::info!(stream_id = %stream_id, "Spawned periodic thumbnail capture task");
}

/// Cancel the periodic thumbnail capture task for a stream.
async fn cancel_thumbnail_task(state: &AppState, stream_id: Uuid) {
    let mut tasks = state.live_tasks.lock().await;
    if let Some(token) = tasks.remove(&stream_id) {
        token.cancel();
        tracing::info!(stream_id = %stream_id, "Cancelled thumbnail capture task");
    }
}

/// Scan filesystem for MP4 recordings, transcode to HLS, optionally upload to GCS.
#[tracing::instrument(skip(uow, recordings_path, storage, config), fields(%stream_id, %stream_key))]
async fn run_transcode(
    uow: repo::UnitOfWork,
    recordings_path: &str,
    stream_id: Uuid,
    stream_key: &str,
    storage: Option<Arc<dyn ObjectStorage>>,
    config: &common::AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stream_dir = PathBuf::from(recordings_path).join(stream_key);

    // Scan filesystem for .mp4 files (don't depend on DB recording table)
    let mp4_files = scan_mp4_files(&stream_dir).await?;

    if mp4_files.is_empty() {
        tracing::warn!(stream_id = %stream_id, dir = %stream_dir.display(), "No MP4 files found, skipping transcode");
        let active = stream::ActiveModel {
            id: Set(stream_id),
            vod_status: Set(stream::VodStatus::None),
            ..Default::default()
        };
        uow.stream_repo().update(active).await?;
        return Ok(());
    }

    // Concat all MP4 segments into one file (single file is a no-op)
    let combined_path = stream_dir.join("combined.mp4");
    let input_mp4 = transcoder::concat_mp4(&mp4_files, &combined_path)
        .await
        .map_err(|e| format!("MP4 concat failed: {e}"))?;
    let output_dir = stream_dir.join("hls");

    tracing::info!(
        stream_id = %stream_id,
        input = %input_mp4.display(),
        output_dir = %output_dir.display(),
        "Starting VOD transcode"
    );

    if config.transcoder_enabled() {
        run_transcode_gcp(
            &uow,
            stream_id,
            stream_key,
            &input_mp4,
            storage.as_deref(),
            config,
        )
        .await?;
    } else {
        run_transcode_local(
            &uow,
            stream_id,
            stream_key,
            &input_mp4,
            &output_dir,
            storage.as_deref(),
        )
        .await?;
    }

    Ok(())
}

/// Scan a directory for .mp4 files, sorted by name (timestamp-based names sort naturally).
async fn scan_mp4_files(
    dir: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("mp4") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Local ffmpeg transcode + optional GCS upload.
#[tracing::instrument(skip(uow, storage), fields(%stream_id, %stream_key))]
async fn run_transcode_local(
    uow: &repo::UnitOfWork,
    stream_id: Uuid,
    stream_key: &str,
    input_mp4: &Path,
    output_dir: &Path,
    storage: Option<&dyn ObjectStorage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match transcoder::transcode_to_hls(input_mp4, output_dir).await {
        Ok(_) => {
            let hls_url = if let Some(store) = storage {
                let gcs_prefix = format!("streams/{}/hls", stream_key);
                store
                    .upload_dir(output_dir, &gcs_prefix)
                    .await
                    .map_err(|e| format!("GCS upload failed: {e}"))?;
                let url = store.public_url(&format!("{}/index.m3u8", gcs_prefix));
                tracing::info!(stream_id = %stream_id, %url, "HLS uploaded to GCS");
                url
            } else {
                format!("/vod/{stream_key}/hls/index.m3u8")
            };

            // Extract thumbnail from the input MP4
            let thumb_path = output_dir.parent().unwrap_or(output_dir).join("thumb.jpg");
            let thumbnail_url = async {
                if let Err(e) = transcoder::extract_thumbnail(input_mp4, &thumb_path).await {
                    tracing::warn!(stream_id = %stream_id, error = %e, "Thumbnail extraction failed");
                    return None;
                }

                let Some(store) = storage else {
                    return Some(format!("/vod/{stream_key}/thumb.jpg"));
                };

                let thumb_key = format!("streams/{}/thumb.jpg", stream_key);
                match store.upload_file(&thumb_path, &thumb_key).await {
                    Ok(_) => {
                        let url = store.public_url(&thumb_key);
                        tracing::info!(stream_id = %stream_id, %url, "Thumbnail uploaded");
                        Some(url)
                    }
                    Err(e) => {
                        tracing::warn!(stream_id = %stream_id, error = %e, "Thumbnail upload failed");
                        None
                    }
                }
            }
            .await;

            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Ready),
                hls_url: Set(Some(hls_url.clone())),
                thumbnail_url: Set(thumbnail_url.clone()),
                ..Default::default()
            };
            uow.stream_repo().update(active).await?;
            tracing::info!(stream_id = %stream_id, %hls_url, ?thumbnail_url, "VOD transcode completed");
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

/// GCP Transcoder API path: upload MP4 to GCS, create transcoder job.
#[tracing::instrument(skip(uow, storage, config), fields(%stream_id, %stream_key))]
async fn run_transcode_gcp(
    uow: &repo::UnitOfWork,
    stream_id: Uuid,
    stream_key: &str,
    input_mp4: &Path,
    storage: Option<&dyn ObjectStorage>,
    config: &common::AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let store = storage.ok_or("GCS storage required for Transcoder API")?;

    // Upload raw MP4 to GCS
    let mp4_key = format!("streams/{}/input.mp4", stream_key);
    store
        .upload_file(input_mp4, &mp4_key)
        .await
        .map_err(|e| format!("GCS upload failed: {e}"))?;

    // Create Transcoder job
    let input_uri = format!("gs://{}/{}", config.gcs_bucket, mp4_key);
    let output_uri = format!("gs://{}/streams/{}/output/", config.gcs_bucket, stream_key);

    let token = transcoder::get_gcp_token().await?;
    transcoder::create_job(
        &config.transcoder_project_id,
        &config.transcoder_location,
        &input_uri,
        &output_uri,
        &stream_id.to_string(),
        &token,
    )
    .await?;

    // Set hls_url and thumbnail_url to expected output paths (confirmed by Pub/Sub webhook)
    let hls_url = format!(
        "https://storage.googleapis.com/{}/streams/{}/output/index.m3u8",
        config.gcs_bucket, stream_key
    );
    let thumbnail_url = format!(
        "https://storage.googleapis.com/{}/streams/{}/output/thumb0000000000.jpeg",
        config.gcs_bucket, stream_key
    );
    let active = stream::ActiveModel {
        id: Set(stream_id),
        hls_url: Set(Some(hls_url)),
        thumbnail_url: Set(Some(thumbnail_url)),
        ..Default::default()
    };
    uow.stream_repo().update(active).await?;

    tracing::info!(stream_id = %stream_id, "Transcoder job created");
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
        super::super::super::tests::test_config()
    }

    fn test_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
        super::super::super::tests::test_metrics()
    }

    fn pending_stream() -> stream::Model {
        let id = Uuid::new_v4();
        stream::Model {
            id,
            user_id: Some(Uuid::new_v4()),
            stream_key: id.to_string(),
            title: Some("Test".to_string()),
            status: stream::StreamStatus::Pending,
            vod_status: stream::VodStatus::None,
            started_at: None,
            ended_at: None,
            created_at: Utc::now(),
            hls_url: None,
            thumbnail_url: None,
        }
    }

    fn live_stream() -> stream::Model {
        let mut s = pending_stream();
        s.status = stream::StreamStatus::Live;
        s.started_at = Some(Utc::now());
        s
    }

    fn app(state: AppState) -> Router {
        Router::new()
            .route("/internal/hooks/publish", post(publish_hook))
            .with_state(state)
    }

    #[tokio::test]
    async fn publish_sets_status_to_live() {
        let s = pending_stream();
        let mut updated = s.clone();
        updated.status = stream::StreamStatus::Live;

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]])
            .append_query_results([vec![updated.clone()]])
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
            storage: None,
            metrics: test_metrics(),
            live_tasks: Default::default(),
        };

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": s.stream_key,
                    "action": "publish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unpublish_sets_status_to_ended_and_processing() {
        let s = live_stream();
        let mut updated = s.clone();
        updated.status = stream::StreamStatus::Ended;
        updated.vod_status = stream::VodStatus::Processing;

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]])
            .append_query_results([vec![updated.clone()]])
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
            storage: None,
            metrics: test_metrics(),
            live_tasks: Default::default(),
        };

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": s.stream_key,
                    "action": "unpublish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn publish_stream_not_found_returns_404() {
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results::<stream::Model, _, _>([vec![]])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
            storage: None,
            metrics: test_metrics(),
            live_tasks: Default::default(),
        };

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": "nonexistent",
                    "action": "publish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
