use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use chrono::Utc;
use entity::stream;
use mediamtx::keys;
use sea_orm::Set;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// JSON body sent by MediaMTX for `runOnPublish` / `runOnUnpublish` hooks.
#[derive(Debug, Deserialize)]
pub struct PublishHookPayload {
    /// MediaMTX path (equals stream key).
    pub stream_key: String,
    /// `"publish"` or `"unpublish"`.
    pub action: String,
}

/// Query string for the publish webhook.
#[derive(Debug, Deserialize)]
pub struct PublishHookQuery {
    /// MediaMTX instance name that sent the webhook (e.g. "mtx-1")
    pub mtx: Option<String>,
    /// Full `$MTX_QUERY` string forwarded from MediaMTX. Because `$MTX_QUERY`
    /// contains `&`, when the webhook URL is built as
    /// `?mtx=...&query=$MTX_QUERY`, axum's Query extractor only captures up to
    /// the first `&` here. The remaining `session=...` leaks out as its own
    /// top-level query param and is caught by `session` below.
    pub query: Option<String>,
    /// Session UUID extracted either from `$MTX_QUERY`'s leaked `session=...`
    /// tail, or supplied directly by tests / callers.
    pub session: Option<Uuid>,
}

/// Extract `session={uuid}` from a `$MTX_QUERY` string. Lenient — unknown params
/// are ignored, missing/invalid session returns None.
fn parse_session_from_query(raw: &str) -> Option<Uuid> {
    for pair in raw.split('&') {
        let mut it = pair.splitn(2, '=');
        let key = it.next()?;
        let value = it.next().unwrap_or("");
        if key == "session" {
            return value.parse().ok();
        }
    }
    None
}

/// `POST /internal/hooks/publish?mtx={name}&query={mtx_query_string}` —
/// MediaMTX publish / unpublish webhook.
///
/// On `publish`: verifies the session is the active one for this stream and
/// came from the expected MTX, flips status to `Live`, increments the MTX
/// stream counter, fans out a live-streams event, and spawns the thumbnail
/// capture task. On `unpublish`: handles the stream migration / stale-session
/// cases and, when current, ends the stream and kicks off VOD transcoding.
///
/// Internal; not exposed outside the cluster.
///
/// # Errors
/// - 404 if the stream key is unknown
/// - 400 for unrecognised `action`
/// - 500 on Redis / DB failure
#[tracing::instrument(skip(state, payload), fields(stream_key = %payload.stream_key, action = %payload.action))]
pub(crate) async fn publish_hook(
    State(state): State<AppState>,
    Query(query): Query<PublishHookQuery>,
    Json(payload): Json<PublishHookPayload>,
) -> Result<StatusCode, StatusCode> {
    let mtx_name = match query.mtx.as_deref() {
        Some(name) => name,
        None => {
            tracing::warn!("Publish hook missing ?mtx= query param, ignoring");
            return Ok(StatusCode::OK);
        }
    };

    // Prefer explicit `session=` (how MediaMTX actually forwards `$MTX_QUERY`
    // through our URL template — see PublishHookQuery docstring), falling back
    // to parsing the `query=` string if present.
    let session_id = query
        .session
        .or_else(|| query.query.as_deref().and_then(parse_session_from_query));

    let Some(session_id) = session_id else {
        tracing::warn!(
            stream_key = %payload.stream_key,
            action = %payload.action,
            mtx = mtx_name,
            raw_query = ?query.query,
            "Publish hook missing/invalid session param — legacy or malformed, ignoring"
        );
        return Ok(StatusCode::OK);
    };

    tracing::info!(
        stream_key = %payload.stream_key,
        action = %payload.action,
        mtx = mtx_name,
        %session_id,
        "Received publish hook"
    );

    // Lookup stream by stream_key (no locking yet — we may bail out as stale).
    let stream = state
        .uow
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
            tracing::warn!(stream_key = %payload.stream_key, "Stream not found for hook");
            return Err(StatusCode::NOT_FOUND);
        }
    };
    let stream_id = stream.id;
    let stream_key = stream.stream_key.clone();

    match payload.action.as_str() {
        "publish" => handle_publish(&state, stream_id, &stream_key, session_id, mtx_name).await,
        "unpublish" => handle_unpublish(&state, stream, session_id, mtx_name).await,
        other => {
            tracing::warn!(action = other, "Unknown hook action");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

async fn handle_publish(
    state: &AppState,
    stream_id: Uuid,
    stream_key: &str,
    session_id: Uuid,
    mtx_name: &str,
) -> Result<StatusCode, StatusCode> {
    // Verify session is still the active one and came from the expected MTX.
    let active = mediamtx::get_active_session(state.cache.as_ref(), &stream_id)
        .await
        .map_err(internal_err)?;
    if active != Some(session_id) {
        tracing::warn!(
            %stream_id,
            %session_id,
            ?active,
            "Stale publish webhook (session superseded), ignoring"
        );
        return Ok(StatusCode::OK);
    }

    let session_mtx = mediamtx::get_session_mtx(state.cache.as_ref(), &session_id)
        .await
        .map_err(internal_err)?;
    if session_mtx.as_deref() != Some(mtx_name) {
        tracing::warn!(
            %session_id,
            expected = ?session_mtx,
            got = mtx_name,
            "Publish webhook MTX mismatch, ignoring"
        );
        return Ok(StatusCode::OK);
    }

    // Flip DB status to Live under row lock.
    let txn = state.uow.begin().await.map_err(internal_err)?;
    let locked = txn
        .stream_repo()
        .find_by_id_for_update(stream_id)
        .await
        .map_err(internal_err)?;
    let Some(locked) = locked else {
        tracing::warn!(%stream_id, "Stream disappeared between lookups");
        return Err(StatusCode::NOT_FOUND);
    };
    let mut active_model: stream::ActiveModel = locked.into();
    active_model.status = Set(stream::StreamStatus::Live);
    active_model.started_at = Set(Some(Utc::now()));
    txn.stream_repo()
        .update(active_model)
        .await
        .map_err(internal_err)?;
    txn.commit().await.map_err(internal_err)?;

    // INCR mtx stream_count.
    if let Err(e) = incr_mtx_count(state, mtx_name).await {
        tracing::error!(error = %e, mtx_name, "Failed to INCR mtx stream_count");
    }

    if let Err(e) = publish_live_streams_event(state).await {
        tracing::error!(error = %e, "Failed to publish live_streams event");
    }

    spawn_thumbnail_task(state, stream_id, stream_key, Some(mtx_name)).await;

    Ok(StatusCode::OK)
}

async fn handle_unpublish(
    state: &AppState,
    stream: stream::Model,
    session_id: Uuid,
    mtx_name: &str,
) -> Result<StatusCode, StatusCode> {
    let stream_id = stream.id;
    let stream_key = stream.stream_key.clone();

    // Does this session exist at all?
    let session_mtx = mediamtx::get_session_mtx(state.cache.as_ref(), &session_id)
        .await
        .map_err(internal_err)?;
    if session_mtx.is_none() {
        tracing::warn!(
            %stream_id,
            %session_id,
            "Unknown session in unpublish webhook — already cleaned up, ignoring"
        );
        return Ok(StatusCode::OK);
    }

    // Is this session the current active one?
    let active = mediamtx::get_active_session(state.cache.as_ref(), &stream_id)
        .await
        .map_err(internal_err)?;

    if active != Some(session_id) {
        // Stale session: stream already migrated to a newer MTX. Just clean up
        // session keys and DECR the original MTX's count. Don't touch DB.
        tracing::info!(
            %stream_id,
            %session_id,
            active_session = ?active,
            stale_mtx = mtx_name,
            "Stale unpublish (stream migrated) — cleaning session only"
        );
        if let Err(e) = mediamtx::cleanup_stale_session(state.cache.as_ref(), &session_id).await {
            tracing::error!(error = %e, "Failed to cleanup stale session");
        }
        return Ok(StatusCode::OK);
    }

    // Active session ending: flip DB to Ended and trigger VOD pipeline.
    let txn = state.uow.begin().await.map_err(internal_err)?;
    let locked = txn
        .stream_repo()
        .find_by_id_for_update(stream_id)
        .await
        .map_err(internal_err)?;
    let Some(locked) = locked else {
        tracing::warn!(%stream_id, "Stream disappeared during unpublish");
        return Err(StatusCode::NOT_FOUND);
    };
    let mut active_model: stream::ActiveModel = locked.into();
    active_model.status = Set(stream::StreamStatus::Ended);
    active_model.ended_at = Set(Some(Utc::now()));
    active_model.vod_status = Set(stream::VodStatus::Processing);
    txn.stream_repo()
        .update(active_model)
        .await
        .map_err(internal_err)?;
    txn.commit().await.map_err(internal_err)?;

    // Tear down session + DECR mtx count.
    if let Err(e) = mediamtx::end_session(state.cache.as_ref(), &session_id).await {
        tracing::error!(error = %e, "Failed to end session");
    }

    // SPEC-026: stop the chat pub/sub forwarder for this stream. Dropping the
    // forwarder closes all local chat subscriber tasks that were spawned by
    // ensure_chat_pubsub_task, so they don't linger as live instances grow.
    if let Err(e) = state
        .pubsub
        .unsubscribe(&mediamtx::keys::chat_pubsub_channel(&stream_id))
        .await
    {
        tracing::warn!(error = %e, "Failed to unsubscribe chat pubsub");
    }

    if let Err(e) = publish_live_streams_event(state).await {
        tracing::error!(error = %e, "Failed to publish live_streams event");
    }

    cancel_thumbnail_task(state, stream_id).await;

    // Fire-and-forget VOD transcode.
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

    Ok(StatusCode::OK)
}

fn internal_err<E: std::fmt::Display>(e: E) -> StatusCode {
    tracing::error!("Internal error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn incr_mtx_count(state: &AppState, mtx_name: &str) -> Result<(), anyhow::Error> {
    let count_key = keys::mtx_stream_count(mtx_name);
    let current: i64 = state
        .cache
        .get(&count_key)
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    state
        .cache
        .set(&count_key, &(current + 1).to_string(), None)
        .await?;
    tracing::info!(mtx_name, new_count = current + 1, "INCR mtx stream_count");
    Ok(())
}

/// Spawn a periodic task that captures a thumbnail from the live HLS stream every 60s.
/// Cancels any previously running task for this stream first.
async fn spawn_thumbnail_task(
    state: &AppState,
    stream_id: Uuid,
    stream_key: &str,
    mtx_name: Option<&str>,
) {
    let token = CancellationToken::new();
    {
        let mut tasks = state.live_tasks.lock().await;
        if let Some(old_token) = tasks.insert(stream_id, token.clone()) {
            old_token.cancel();
            tracing::info!(%stream_id, "Cancelled old thumbnail task before spawning new one");
        }
    }

    let uow = state.uow.clone();
    let storage = state.storage.clone();
    let thumbnails_path = state.config.thumbnails_path.clone();
    let capture_interval = state.config.thumbnail_capture_interval_secs;
    let stream_key = stream_key.to_string();
    let state_clone = state.clone();

    let hls_base = mtx_name
        .and_then(|name| mediamtx::find_instance(&state.mtx_instances, name))
        .map(|inst| inst.internal_api.replace(":9997", ":8888"))
        .unwrap_or_else(|| "http://mediamtx:8888".to_string());

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(capture_interval));
        let mut first_capture_done = false;
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::info!(stream_id = %stream_id, "Thumbnail capture task cancelled");
                    break;
                }
                _ = interval.tick() => {
                    let hls_url = format!("{}/{}/index.m3u8", hls_base, stream_key);
                    let thumb_dir = PathBuf::from(&thumbnails_path).join(&stream_key);
                    let thumb_path = thumb_dir.join("live-thumb.jpg");

                    match transcoder::capture_hls_thumbnail(&hls_url, &thumb_path).await {
                        Ok(_) => {
                            let key = format!("streams/{}/live-thumb.jpg", stream_key);
                            let thumbnail_url = match storage.upload_file(&thumb_path, &key).await {
                                Ok(_) => storage.public_url(&key),
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to upload thumbnail to storage");
                                    continue;
                                }
                            };

                            let active = stream::ActiveModel {
                                id: Set(stream_id),
                                thumbnail_url: Set(Some(thumbnail_url)),
                                ..Default::default()
                            };
                            if let Err(e) = uow.stream_repo().update(active).await {
                                tracing::warn!(error = %e, "Failed to update thumbnail_url in DB");
                            }

                            if !first_capture_done {
                                if let Err(e) = publish_live_streams_event(&state_clone).await {
                                    tracing::warn!(error = %e, "Failed to publish live streams after thumbnail");
                                }
                                first_capture_done = true;
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

/// Publish the current live-streams list on Redis `streamhub:events` so every
/// API instance can push an updated snapshot to its WebSocket clients.
pub(crate) async fn publish_live_streams_event(state: &AppState) -> Result<(), anyhow::Error> {
    let live_models = state.uow.stream_repo().list_live().await?;
    let mut data = Vec::with_capacity(live_models.len());

    for m in live_models {
        let urls = mediamtx::resolve_stream_urls(
            state.cache.as_ref(),
            &state.mtx_instances,
            &m.id,
            &m.stream_key,
        )
        .await
        .unwrap_or(None);

        let (whep, hls) = match urls {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };

        data.push(crate::ws::types::LiveStreamData {
            id: m.id,
            title: m.title,
            stream_key: m.stream_key,
            status: serde_json::to_value(&m.status)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "unknown".to_string()),
            thumbnail_url: m.thumbnail_url,
            started_at: m.started_at,
            viewer_count: 0,
            urls: crate::ws::types::LiveStreamUrls { whep, hls },
        });
    }

    let event = crate::ws::types::RedisEvent::LiveStreams { data };
    let json = serde_json::to_string(&event)?;
    state.pubsub.publish("streamhub:events", &json).await?;
    Ok(())
}

/// Scan filesystem for MP4 recordings, transcode to HLS, optionally upload to GCS.
#[tracing::instrument(skip(uow, recordings_path, storage, config), fields(%stream_id, %stream_key))]
async fn run_transcode(
    uow: repo::UnitOfWork,
    recordings_path: &str,
    stream_id: Uuid,
    stream_key: &str,
    storage: Arc<dyn storage::ObjectStorage>,
    config: &crate::config::AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stream_dir = PathBuf::from(recordings_path).join(stream_key);

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
        run_transcode_gcp(&uow, stream_id, stream_key, &input_mp4, &*storage, config).await?;
    } else {
        run_transcode_local(
            &uow,
            stream_id,
            stream_key,
            &input_mp4,
            &output_dir,
            &*storage,
        )
        .await?;
    }

    Ok(())
}

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

#[tracing::instrument(skip(uow, storage), fields(%stream_id, %stream_key))]
async fn run_transcode_local(
    uow: &repo::UnitOfWork,
    stream_id: Uuid,
    stream_key: &str,
    input_mp4: &Path,
    output_dir: &Path,
    storage: &dyn storage::ObjectStorage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match transcoder::transcode_to_hls(input_mp4, output_dir).await {
        Ok(_) => {
            let gcs_prefix = format!("streams/{}/hls", stream_key);
            storage
                .upload_dir(output_dir, &gcs_prefix)
                .await
                .map_err(|e| format!("GCS upload failed: {e}"))?;
            let hls_url = storage.public_url(&format!("{}/index.m3u8", gcs_prefix));
            tracing::info!(stream_id = %stream_id, %hls_url, "HLS uploaded to storage");

            let thumb_path = output_dir.parent().unwrap_or(output_dir).join("thumb.jpg");
            let thumbnail_url = async {
                if let Err(e) = transcoder::extract_thumbnail(input_mp4, &thumb_path).await {
                    tracing::warn!(stream_id = %stream_id, error = %e, "Thumbnail extraction failed");
                    return None;
                }

                let thumb_key = format!("streams/{}/thumb.jpg", stream_key);
                match storage.upload_file(&thumb_path, &thumb_key).await {
                    Ok(_) => {
                        let url = storage.public_url(&thumb_key);
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

#[tracing::instrument(skip(uow, storage, config), fields(%stream_id, %stream_key))]
async fn run_transcode_gcp(
    uow: &repo::UnitOfWork,
    stream_id: Uuid,
    stream_key: &str,
    input_mp4: &Path,
    storage: &dyn storage::ObjectStorage,
    config: &crate::config::AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let store = storage;

    let mp4_key = format!("streams/{}/input.mp4", stream_key);
    store
        .upload_file(input_mp4, &mp4_key)
        .await
        .map_err(|e| format!("GCS upload failed: {e}"))?;

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
    use crate::config::AppConfig;
    use crate::state::AppState;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use cache::CacheStore;
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

    fn base_state(db: sea_orm::DatabaseConnection, cache: Arc<dyn CacheStore>) -> AppState {
        AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
            storage: crate::tests::test_storage(),
            metrics: test_metrics(),
            redis_pool: crate::tests::test_redis_pool(),
            cache,
            pubsub: crate::tests::test_pubsub(),
            live_tasks: Default::default(),
            mtx_instances: vec![],
        }
    }

    #[test]
    fn parse_session_extracts_uuid() {
        let sid = Uuid::new_v4();
        let q = format!("token=abc&session={sid}&extra=1");
        assert_eq!(parse_session_from_query(&q), Some(sid));
        assert_eq!(parse_session_from_query("token=abc"), None);
        assert_eq!(parse_session_from_query("session=not-a-uuid"), None);
    }

    #[tokio::test]
    async fn publish_with_active_session_sets_live() {
        let s = pending_stream();
        let stream_id = s.id;
        let mtx_name = "mtx-1";

        let cache: Arc<dyn CacheStore> = Arc::new(cache::InMemoryCache::new());
        let session_id = mediamtx::create_session(cache.as_ref(), &stream_id, mtx_name)
            .await
            .unwrap();

        // find_by_key then find_by_id_for_update (inside txn) then update
        let mut updated = s.clone();
        updated.status = stream::StreamStatus::Live;
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]])
            .append_query_results([vec![s.clone()]])
            .append_query_results([vec![updated.clone()]])
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = base_state(db, cache);

        let uri = format!("/internal/hooks/publish?mtx={mtx_name}&query=session%3D{session_id}");
        let req = Request::builder()
            .method("POST")
            .uri(uri)
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
    async fn publish_with_stale_session_ignored() {
        let s = pending_stream();
        let stream_id = s.id;
        let mtx_name = "mtx-1";

        let cache: Arc<dyn CacheStore> = Arc::new(cache::InMemoryCache::new());
        // Active session is a different uuid.
        let _active = mediamtx::create_session(cache.as_ref(), &stream_id, mtx_name)
            .await
            .unwrap();
        let stale_sid = Uuid::new_v4();

        // Only expect the initial find_by_key; no update should follow.
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]])
            .into_connection();

        let state = base_state(db, cache);

        let uri = format!("/internal/hooks/publish?mtx={mtx_name}&query=session%3D{stale_sid}");
        let req = Request::builder()
            .method("POST")
            .uri(uri)
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
    async fn unpublish_stale_session_does_not_end_stream() {
        let s = live_stream();
        let stream_id = s.id;
        let mtx_name = "mtx-old";

        let cache: Arc<dyn CacheStore> = Arc::new(cache::InMemoryCache::new());
        // Stale session belongs to mtx-old but active is now mtx-new.
        let stale_sid = mediamtx::create_session(cache.as_ref(), &stream_id, mtx_name)
            .await
            .unwrap();
        // Simulate migration by creating a newer active session.
        let _new = mediamtx::create_session(cache.as_ref(), &stream_id, "mtx-new")
            .await
            .unwrap();

        // Only expect find_by_key — no DB update path.
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]])
            .into_connection();

        let state = base_state(db, cache.clone());

        let uri = format!("/internal/hooks/publish?mtx={mtx_name}&query=session%3D{stale_sid}");
        let req = Request::builder()
            .method("POST")
            .uri(uri)
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

        // stale session keys should be gone
        assert!(
            cache
                .get(&mediamtx::keys::session_mtx(&stale_sid))
                .await
                .unwrap()
                .is_none()
        );
        // active_session still points to the newer session, untouched
        assert!(
            cache
                .get(&mediamtx::keys::stream_active_session(&stream_id))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn publish_without_session_returns_ok_and_does_nothing() {
        let db = MockDatabase::new(DbBackend::Postgres).into_connection();
        let state = base_state(db, Arc::new(cache::InMemoryCache::new()));

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish?mtx=mtx-1")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": "abc",
                    "action": "publish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
