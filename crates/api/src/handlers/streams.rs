use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::Utc;
use entity::stream;
use entity::user::UserRole;
use error::AppError;
use sea_orm::Set;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::extractors::AppJson;

use crate::middleware::CurrentUser;

/// Request body for `POST /v1/streams`.
#[derive(Debug, Deserialize)]
pub struct CreateStreamRequest {
    /// Optional display title for the stream.
    pub title: Option<String>,
}

/// Request body for `PATCH /v1/streams/:id`.
#[derive(Debug, Deserialize)]
pub struct UpdateStreamRequest {
    /// New display title. Omit to leave unchanged.
    pub title: Option<String>,
}

/// Query string for `GET /v1/streams`.
#[derive(Debug, Deserialize)]
pub struct ListStreamsQuery {
    /// Filter by status (`Pending` / `Live` / `Ended` / `Error`).
    pub status: Option<String>,
    /// 1-indexed page number. Defaults to 1.
    pub page: Option<u64>,
    /// Page size, capped at 100. Defaults to 20.
    pub per_page: Option<u64>,
}

/// Full stream representation returned by the `/v1/streams/*` endpoints.
#[derive(Debug, Serialize)]
pub struct StreamResponse {
    /// Stream UUID.
    pub id: Uuid,
    /// Owning broadcaster UUID (nullable for orphaned / legacy rows).
    pub user_id: Option<Uuid>,
    /// MediaMTX path key (UUID v4 format).
    pub stream_key: String,
    /// Display title.
    pub title: Option<String>,
    /// Live/ended state machine status.
    pub status: stream::StreamStatus,
    /// VOD post-processing status (transcode pipeline).
    pub vod_status: stream::VodStatus,
    /// Public VOD playlist URL (set once transcoding finishes).
    pub hls_url: Option<String>,
    /// Poster/thumbnail URL (live or VOD).
    pub thumbnail_url: Option<String>,
    /// Ephemeral playback URLs resolved against the routing MTX instance.
    pub urls: StreamUrls,
    /// Time the stream went live.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Time the stream ended.
    pub ended_at: Option<chrono::DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: chrono::DateTime<Utc>,
}

/// Playback / publish URLs that change based on which MediaMTX instance a
/// stream is currently routed to. All fields are `None` when the stream is
/// not live.
#[derive(Debug, Serialize)]
pub struct StreamUrls {
    /// WHIP publish endpoint (broadcasters). Currently always `None` in the
    /// list response — issued by `POST /v1/streams/:id/token` instead.
    pub whip: Option<String>,
    /// WHEP playback endpoint for low-latency viewers.
    pub whep: Option<String>,
    /// LL-HLS playlist URL for mass playback.
    pub hls: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DataResponse<T: Serialize> {
    pub(crate) data: T,
}

#[derive(Debug, Serialize)]
pub(crate) struct PaginatedResponse<T: Serialize> {
    pub(crate) data: Vec<T>,
    pub(crate) pagination: Pagination,
}

#[derive(Debug, Serialize)]
pub(crate) struct Pagination {
    page: u64,
    per_page: u64,
    total: u64,
    total_pages: u64,
}

/// Compact stream representation used by the live and VOD listing endpoints.
///
/// Omits owner-specific / creation metadata for lighter payloads on the
/// public home page.
#[derive(Debug, Serialize)]
pub struct LiveStreamResponse {
    /// Stream UUID.
    pub id: Uuid,
    /// MediaMTX path key.
    pub stream_key: String,
    /// Display title.
    pub title: Option<String>,
    /// Stream lifecycle status.
    pub status: stream::StreamStatus,
    /// VOD post-processing status.
    pub vod_status: stream::VodStatus,
    /// Public VOD playlist URL.
    pub hls_url: Option<String>,
    /// Poster/thumbnail URL.
    pub thumbnail_url: Option<String>,
    /// Time the stream went live.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Time the stream ended.
    pub ended_at: Option<chrono::DateTime<Utc>>,
    /// Ephemeral playback URLs.
    pub urls: StreamUrls,
}

async fn resolve_urls(state: &AppState, model: &stream::Model) -> StreamUrls {
    if model.status == stream::StreamStatus::Live {
        // For live streams, resolve URLs from the stream's active session.
        match mediamtx::resolve_stream_urls(
            state.cache.as_ref(),
            &state.mtx_instances,
            &model.id,
            &model.stream_key,
        )
        .await
        {
            Ok(Some((whep, hls))) => StreamUrls {
                whip: None,
                whep: Some(whep),
                hls: Some(hls),
            },
            _ => StreamUrls {
                whip: None,
                whep: None,
                hls: None,
            },
        }
    } else {
        StreamUrls {
            whip: None,
            whep: None,
            hls: None,
        }
    }
}

async fn build_live_stream_response(state: &AppState, model: stream::Model) -> LiveStreamResponse {
    let urls = resolve_urls(state, &model).await;
    LiveStreamResponse {
        id: model.id,
        stream_key: model.stream_key.clone(),
        title: model.title,
        status: model.status,
        vod_status: model.vod_status,
        hls_url: model.hls_url,
        thumbnail_url: model.thumbnail_url,
        started_at: model.started_at,
        ended_at: model.ended_at,
        urls,
    }
}

async fn build_stream_response(state: &AppState, model: stream::Model) -> StreamResponse {
    let urls = resolve_urls(state, &model).await;
    StreamResponse {
        id: model.id,
        user_id: model.user_id,
        stream_key: model.stream_key.clone(),
        title: model.title,
        status: model.status,
        vod_status: model.vod_status,
        hls_url: model.hls_url,
        thumbnail_url: model.thumbnail_url,
        urls,
        started_at: model.started_at,
        ended_at: model.ended_at,
        created_at: model.created_at,
    }
}

/// `GET /v1/streams/vod` — list streams with a ready VOD asset.
///
/// Public (no auth). Used by the viewer index page.
///
/// # Errors
/// - 500 on DB failure
#[tracing::instrument(skip(state))]
pub(crate) async fn list_vod_streams(
    State(state): State<AppState>,
) -> Result<Json<DataResponse<Vec<LiveStreamResponse>>>, AppError> {
    let models = state.uow.stream_repo().list_vod().await?;

    let mut data = Vec::with_capacity(models.len());
    for m in models {
        data.push(build_live_stream_response(&state, m).await);
    }

    Ok(Json(DataResponse { data }))
}

/// `GET /v1/streams/live` — list currently live streams with resolved WHEP/HLS URLs.
///
/// Public (no auth).
///
/// # Errors
/// - 500 on DB failure
#[tracing::instrument(skip(state))]
pub(crate) async fn list_live_streams(
    State(state): State<AppState>,
) -> Result<Json<DataResponse<Vec<LiveStreamResponse>>>, AppError> {
    let models = state.uow.stream_repo().list_live().await?;

    let mut data = Vec::with_capacity(models.len());
    for m in models {
        data.push(build_live_stream_response(&state, m).await);
    }

    Ok(Json(DataResponse { data }))
}

/// `POST /v1/streams` — create a new stream row in `Pending`.
///
/// Returns 201 with `StreamResponse`. The stream key is the row's UUID v4.
///
/// # Errors
/// - 401 if unauthenticated
/// - 403 if the caller is not a broadcaster/admin
/// - 500 on DB failure
#[tracing::instrument(skip(state, payload), fields(user_id = %current_user.id))]
pub(crate) async fn create_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    AppJson(payload): AppJson<CreateStreamRequest>,
) -> Result<(StatusCode, Json<DataResponse<StreamResponse>>), AppError> {
    if current_user.role != UserRole::Broadcaster && current_user.role != UserRole::Admin {
        return Err(AppError::Forbidden(
            "only broadcasters can create streams".to_string(),
        ));
    }

    let id = Uuid::new_v4();
    let stream_key = id.to_string();

    let active = stream::ActiveModel {
        id: Set(id),
        user_id: Set(Some(current_user.id)),
        stream_key: Set(stream_key),
        title: Set(payload.title),
        status: Set(stream::StreamStatus::Pending),
        vod_status: Set(stream::VodStatus::None),
        started_at: Set(None),
        ended_at: Set(None),
        created_at: Set(Utc::now()),
        hls_url: Set(None),
        thumbnail_url: Set(None),
    };

    let txn = state.uow.begin().await?;
    let model = txn.stream_repo().create(active).await?;
    txn.commit().await?;

    let resp = build_stream_response(&state, model).await;

    Ok((StatusCode::CREATED, Json(DataResponse { data: resp })))
}

/// `GET /v1/streams` — paginated list of the caller's own streams.
///
/// # Errors
/// - 400 on invalid `status` filter
/// - 401 if unauthenticated
/// - 500 on DB failure
#[tracing::instrument(skip(state, params), fields(user_id = %current_user.id))]
pub(crate) async fn list_streams(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Query(params): Query<ListStreamsQuery>,
) -> Result<Json<PaginatedResponse<StreamResponse>>, AppError> {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).min(100);

    let status_filter = if let Some(status) = &params.status {
        Some(match status.as_str() {
            "Pending" => stream::StreamStatus::Pending,
            "Live" => stream::StreamStatus::Live,
            "Ended" => stream::StreamStatus::Ended,
            "Error" => stream::StreamStatus::Error,
            _ => return Err(AppError::BadRequest("invalid status filter".to_string())),
        })
    } else {
        None
    };

    let result = state
        .uow
        .stream_repo()
        .list_by_user(current_user.id, status_filter, page, per_page)
        .await?;

    let total_pages = result.total.div_ceil(per_page);

    let mut data = Vec::with_capacity(result.items.len());
    for m in result.items {
        data.push(build_stream_response(&state, m).await);
    }

    Ok(Json(PaginatedResponse {
        data,
        pagination: Pagination {
            page,
            per_page,
            total: result.total,
            total_pages,
        },
    }))
}

/// `GET /v1/streams/:id` — fetch a single stream by id.
///
/// Public (no auth).
///
/// # Errors
/// - 404 `STREAM_NOT_FOUND`
/// - 500 on DB failure
#[tracing::instrument(skip(state), fields(stream_id = %id))]
pub(crate) async fn get_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let model = state
        .uow
        .stream_repo()
        .find_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let resp = build_stream_response(&state, model).await;
    Ok(Json(DataResponse { data: resp }))
}

/// `PATCH /v1/streams/:id` — update mutable stream fields (currently `title`).
///
/// # Errors
/// - 401 if unauthenticated
/// - 403 if the caller is not the stream owner
/// - 404 `STREAM_NOT_FOUND`
/// - 500 on DB failure
#[tracing::instrument(skip(state, payload), fields(stream_id = %id, user_id = %current_user.id))]
pub(crate) async fn update_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    AppJson(payload): AppJson<UpdateStreamRequest>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let txn = state.uow.begin().await?;

    let existing = txn
        .stream_repo()
        .find_by_id_for_update(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if existing.user_id != Some(current_user.id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

    let mut active = stream::ActiveModel {
        id: Set(id),
        ..Default::default()
    };
    if let Some(title) = payload.title {
        active.title = Set(Some(title));
    }

    let model = txn.stream_repo().update(active).await?;
    txn.commit().await?;

    let resp = build_stream_response(&state, model).await;
    Ok(Json(DataResponse { data: resp }))
}

/// `DELETE /v1/streams/:id` — delete a stream that is not currently live.
///
/// Returns 204 on success.
///
/// # Errors
/// - 401 if unauthenticated
/// - 403 if the caller is not the stream owner
/// - 404 `STREAM_NOT_FOUND`
/// - 409 `STREAM_CANNOT_DELETE` if status is `Live`
/// - 500 on DB failure
#[tracing::instrument(skip(state), fields(stream_id = %id, user_id = %current_user.id))]
pub(crate) async fn delete_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let txn = state.uow.begin().await?;

    let existing = txn
        .stream_repo()
        .find_by_id_for_update(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if existing.user_id != Some(current_user.id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

    if existing.status == stream::StreamStatus::Live {
        return Err(AppError::Conflict("STREAM_CANNOT_DELETE".to_string()));
    }

    txn.stream_repo().delete(id).await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/streams/:id/end` — owner-initiated transition from `Live` to `Ended`.
///
/// Useful when the publisher disconnects uncleanly and MediaMTX never fires
/// the unpublish webhook.
///
/// # Errors
/// - 401 if unauthenticated
/// - 403 if the caller is not the stream owner
/// - 404 `STREAM_NOT_FOUND`
/// - 409 `STREAM_NOT_LIVE` if the stream is not currently `Live`
/// - 500 on DB failure
#[tracing::instrument(skip(state), fields(stream_id = %id, user_id = %current_user.id))]
pub(crate) async fn end_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let txn = state.uow.begin().await?;

    let existing = txn
        .stream_repo()
        .find_by_id_for_update(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if existing.user_id != Some(current_user.id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

    if existing.status != stream::StreamStatus::Live {
        return Err(AppError::Conflict("STREAM_NOT_LIVE".to_string()));
    }

    let active = stream::ActiveModel {
        id: Set(id),
        status: Set(stream::StreamStatus::Ended),
        ended_at: Set(Some(Utc::now())),
        ..Default::default()
    };

    let model = txn.stream_repo().update(active).await?;
    txn.commit().await?;

    let resp = build_stream_response(&state, model).await;
    Ok(Json(DataResponse { data: resp }))
}

/// Response for `POST /v1/streams/:id/token`.
#[derive(Debug, Serialize)]
pub struct StreamTokenResponse {
    /// Raw stream token (only value the client ever sees; stored hashed in Redis).
    pub token: String,
    /// Token expiry instant.
    pub expires_at: chrono::DateTime<Utc>,
    /// Fully-qualified WHIP URL the broadcaster should publish to; includes
    /// the token and session query params.
    pub whip_url: String,
}

const STREAM_TOKEN_TTL_SECS: u64 = 3600;

/// `POST /v1/streams/:id/token` — issue a one-hour publish token and bind the
/// stream to a healthy MediaMTX instance.
///
/// Picks the least-loaded MTX, creates a session row, persists the hashed
/// token in Redis with TTL auto-cleanup, and returns the raw token plus the
/// full WHIP URL the broadcaster should use.
///
/// # Errors
/// - 401 if unauthenticated
/// - 403 if the caller is not a broadcaster/admin or not the stream owner
/// - 404 `STREAM_NOT_FOUND`
/// - 500 when no healthy MTX instance is available, session creation fails,
///   or Redis writes fail
#[tracing::instrument(skip(state), fields(stream_id = %id, user_id = %current_user.id))]
pub(crate) async fn create_stream_token(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<DataResponse<StreamTokenResponse>>), AppError> {
    if current_user.role != UserRole::Broadcaster && current_user.role != UserRole::Admin {
        return Err(AppError::Forbidden(
            "only broadcasters can create stream tokens".to_string(),
        ));
    }

    let existing = state
        .uow
        .stream_repo()
        .find_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if existing.user_id != Some(current_user.id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

    if state
        .cache
        .get(&mediamtx::keys::stream_force_ended(&id))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .is_some()
    {
        return Err(AppError::Conflict("STREAM_FORCE_ENDED".to_string()));
    }

    // Select a MediaMTX instance for this stream
    let instance = mediamtx::select_instance(state.cache.as_ref(), &state.mtx_instances)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "No healthy MTX instance available");
            AppError::Internal("no healthy MediaMTX instance available".to_string())
        })?;

    // Create a session for this publish attempt. The publish webhook will verify
    // the session_id it receives matches stream:{id}:active_session before
    // incrementing the MTX count and flipping status to Live.
    let session_id = mediamtx::create_session(state.cache.as_ref(), &id, &instance.name)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create stream session");
            AppError::Internal("failed to record stream routing".to_string())
        })?;

    // Cache the stream owner so chat ban checks can resolve stream → broadcaster
    // without hitting the DB on every message.
    state
        .cache
        .set(
            &mediamtx::keys::stream_owner(&id),
            &current_user.id.to_string(),
            None, // no TTL — lives as long as the stream is relevant
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to cache stream owner");
            AppError::Internal("failed to cache stream owner".to_string())
        })?;

    // Generate raw token, hash it, and store hash → stream_id in Redis with TTL auto-cleanup.
    let raw_token = auth::token::generate_stream_token();
    let token_hash = auth::token::hash_token(&raw_token);
    let expires_at = Utc::now() + chrono::Duration::seconds(STREAM_TOKEN_TTL_SECS as i64);

    state
        .cache
        .set(
            &mediamtx::keys::stream_token(&token_hash),
            &id.to_string(),
            Some(STREAM_TOKEN_TTL_SECS),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to persist stream token");
            AppError::Internal("failed to persist stream token".to_string())
        })?;

    let whip_url = format!(
        "{}/{}/whip?token={raw_token}&session={session_id}",
        instance.public_whip, existing.stream_key
    );

    Ok((
        StatusCode::CREATED,
        Json(DataResponse {
            data: StreamTokenResponse {
                token: raw_token,
                expires_at,
                whip_url,
            },
        }),
    ))
}

/// A single recorded segment produced by MediaMTX.
#[derive(Debug, Serialize)]
pub struct RecordingResponse {
    /// Recording UUID.
    pub id: Uuid,
    /// Owning stream UUID.
    pub stream_id: Uuid,
    /// Storage path (container-internal path reported by MediaMTX).
    pub file_path: String,
    /// Segment duration in seconds, if known.
    pub duration_secs: Option<i64>,
    /// File size in bytes, if readable when the hook fired.
    pub file_size_bytes: Option<i64>,
    /// Segment creation timestamp.
    pub created_at: chrono::DateTime<Utc>,
}

fn build_recording_response(model: entity::recording::Model) -> RecordingResponse {
    RecordingResponse {
        id: model.id,
        stream_id: model.stream_id,
        file_path: model.file_path,
        duration_secs: model.duration_secs,
        file_size_bytes: model.file_size_bytes,
        created_at: model.created_at,
    }
}

/// Query string for `GET /v1/streams/:id/recordings`.
#[derive(Debug, Deserialize)]
pub struct ListRecordingsQuery {
    /// 1-indexed page number. Defaults to 1.
    pub page: Option<u64>,
    /// Page size, capped at 100. Defaults to 20.
    pub per_page: Option<u64>,
}

/// `GET /v1/streams/:id/recordings` — paginated list of recorded segments.
///
/// # Errors
/// - 404 `STREAM_NOT_FOUND`
/// - 500 on DB failure
#[tracing::instrument(skip(state, params), fields(stream_id = %id))]
pub(crate) async fn list_recordings(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ListRecordingsQuery>,
) -> Result<Json<PaginatedResponse<RecordingResponse>>, AppError> {
    // Verify stream exists
    state
        .uow
        .stream_repo()
        .find_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).min(100);

    let result = state
        .uow
        .recording_repo()
        .list_by_stream(id, page, per_page)
        .await?;
    let total_pages = result.total.div_ceil(per_page);

    let data: Vec<RecordingResponse> = result
        .items
        .into_iter()
        .map(build_recording_response)
        .collect();

    Ok(Json(PaginatedResponse {
        data,
        pagination: Pagination {
            page,
            per_page,
            total: result.total,
            total_pages,
        },
    }))
}
