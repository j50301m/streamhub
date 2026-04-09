use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use common::{AppError, AppState};
use entity::user::UserRole;
use entity::{recording, stream};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::stream_token;

use crate::middleware::CurrentUser;

#[derive(Debug, Deserialize)]
pub struct CreateStreamRequest {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStreamRequest {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListStreamsQuery {
    pub status: Option<String>,
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct StreamResponse {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub stream_key: String,
    pub title: Option<String>,
    pub status: stream::StreamStatus,
    pub vod_status: stream::VodStatus,
    pub hls_url: Option<String>,
    pub urls: StreamUrls,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct StreamUrls {
    pub whip: String,
    pub whep: String,
    pub hls: String,
}

#[derive(Debug, Serialize)]
struct DataResponse<T: Serialize> {
    data: T,
}

#[derive(Debug, Serialize)]
struct PaginatedResponse<T: Serialize> {
    data: Vec<T>,
    pagination: Pagination,
}

#[derive(Debug, Serialize)]
struct Pagination {
    page: u64,
    per_page: u64,
    total: u64,
    total_pages: u64,
}

#[derive(Debug, Serialize)]
pub struct LiveStreamResponse {
    pub id: Uuid,
    pub stream_key: String,
    pub title: Option<String>,
    pub status: stream::StreamStatus,
    pub vod_status: stream::VodStatus,
    pub hls_url: Option<String>,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub urls: StreamUrls,
}

fn build_live_stream_response(model: stream::Model, mediamtx_base: &str) -> LiveStreamResponse {
    let key = &model.stream_key;
    LiveStreamResponse {
        id: model.id,
        stream_key: model.stream_key.clone(),
        title: model.title,
        status: model.status,
        vod_status: model.vod_status,
        hls_url: model.hls_url,
        started_at: model.started_at,
        ended_at: model.ended_at,
        urls: StreamUrls {
            whip: format!("{mediamtx_base}/{key}/whip"),
            whep: format!("{mediamtx_base}/{key}/whep"),
            hls: format!("{mediamtx_base}/{key}/index.m3u8"),
        },
    }
}

fn build_stream_response(model: stream::Model, mediamtx_base: &str) -> StreamResponse {
    let key = &model.stream_key;
    StreamResponse {
        id: model.id,
        user_id: model.user_id,
        stream_key: model.stream_key.clone(),
        title: model.title,
        status: model.status,
        vod_status: model.vod_status,
        hls_url: model.hls_url,
        urls: StreamUrls {
            whip: format!("{mediamtx_base}/{key}/whip"),
            whep: format!("{mediamtx_base}/{key}/whep"),
            hls: format!("{mediamtx_base}/{key}/index.m3u8"),
        },
        started_at: model.started_at,
        ended_at: model.ended_at,
        created_at: model.created_at,
    }
}

/// GET /v1/streams/vod — public, returns ended streams with VOD ready
async fn list_vod_streams(
    State(state): State<AppState>,
) -> Result<Json<DataResponse<Vec<LiveStreamResponse>>>, AppError> {
    let models = stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Ended))
        .filter(stream::Column::VodStatus.eq(stream::VodStatus::Ready))
        .order_by_desc(stream::Column::EndedAt)
        .all(&state.db)
        .await?;

    let data: Vec<LiveStreamResponse> = models
        .into_iter()
        .map(|m| build_live_stream_response(m, &state.mediamtx_url))
        .collect();

    Ok(Json(DataResponse { data }))
}

/// GET /v1/streams/live — public, returns all live streams
async fn list_live_streams(
    State(state): State<AppState>,
) -> Result<Json<DataResponse<Vec<LiveStreamResponse>>>, AppError> {
    let models = stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Live))
        .order_by_desc(stream::Column::StartedAt)
        .all(&state.db)
        .await?;

    let data: Vec<LiveStreamResponse> = models
        .into_iter()
        .map(|m| build_live_stream_response(m, &state.mediamtx_url))
        .collect();

    Ok(Json(DataResponse { data }))
}

/// POST /v1/streams — requires auth (broadcaster role)
async fn create_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Json(payload): Json<CreateStreamRequest>,
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
    };

    let model = active.insert(&state.db).await?;
    let resp = build_stream_response(model, &state.mediamtx_url);

    Ok((StatusCode::CREATED, Json(DataResponse { data: resp })))
}

/// GET /v1/streams — requires auth, returns user's own streams
async fn list_streams(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Query(params): Query<ListStreamsQuery>,
) -> Result<Json<PaginatedResponse<StreamResponse>>, AppError> {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).min(100);

    let mut query = stream::Entity::find().filter(stream::Column::UserId.eq(current_user.id));

    if let Some(status) = &params.status {
        let s = match status.as_str() {
            "Pending" => stream::StreamStatus::Pending,
            "Live" => stream::StreamStatus::Live,
            "Ended" => stream::StreamStatus::Ended,
            "Error" => stream::StreamStatus::Error,
            _ => return Err(AppError::BadRequest("invalid status filter".to_string())),
        };
        query = query.filter(stream::Column::Status.eq(s));
    }

    let total = query.clone().count(&state.db).await?;
    let total_pages = total.div_ceil(per_page);

    let models = query
        .order_by_desc(stream::Column::CreatedAt)
        .paginate(&state.db, per_page)
        .fetch_page(page - 1)
        .await?;

    let data: Vec<StreamResponse> = models
        .into_iter()
        .map(|m| build_stream_response(m, &state.mediamtx_url))
        .collect();

    Ok(Json(PaginatedResponse {
        data,
        pagination: Pagination {
            page,
            per_page,
            total,
            total_pages,
        },
    }))
}

/// GET /v1/streams/:id — public
async fn get_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let model = stream::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let resp = build_stream_response(model, &state.mediamtx_url);
    Ok(Json(DataResponse { data: resp }))
}

/// Helper to load stream and verify ownership.
async fn load_owned_stream(
    state: &AppState,
    stream_id: Uuid,
    user_id: Uuid,
) -> Result<stream::Model, AppError> {
    let model = stream::Entity::find_by_id(stream_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if model.user_id != Some(user_id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

    Ok(model)
}

/// PATCH /v1/streams/:id — owner only
async fn update_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateStreamRequest>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let _existing = load_owned_stream(&state, id, current_user.id).await?;

    let mut active = stream::ActiveModel {
        id: Set(id),
        ..Default::default()
    };
    if let Some(title) = payload.title {
        active.title = Set(Some(title));
    }

    let model = active.update(&state.db).await?;
    let resp = build_stream_response(model, &state.mediamtx_url);
    Ok(Json(DataResponse { data: resp }))
}

/// DELETE /v1/streams/:id — owner only, not allowed if Live
async fn delete_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let existing = load_owned_stream(&state, id, current_user.id).await?;

    if existing.status == stream::StreamStatus::Live {
        return Err(AppError::Conflict("STREAM_CANNOT_DELETE".to_string()));
    }

    stream::Entity::delete_by_id(id).exec(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /v1/streams/:id/end — owner only, must be Live
async fn end_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let existing = load_owned_stream(&state, id, current_user.id).await?;

    if existing.status != stream::StreamStatus::Live {
        return Err(AppError::Conflict("STREAM_NOT_LIVE".to_string()));
    }

    let active = stream::ActiveModel {
        id: Set(id),
        status: Set(stream::StreamStatus::Ended),
        ended_at: Set(Some(Utc::now())),
        ..Default::default()
    };

    let model = active.update(&state.db).await?;
    let resp = build_stream_response(model, &state.mediamtx_url);
    Ok(Json(DataResponse { data: resp }))
}

#[derive(Debug, Serialize)]
pub struct StreamTokenResponse {
    pub token: String,
    pub expires_at: chrono::DateTime<Utc>,
    pub whip_url: String,
}

/// POST /v1/streams/:id/token — owner + broadcaster only, generates a 1hr push token
async fn create_stream_token(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<DataResponse<StreamTokenResponse>>), AppError> {
    if current_user.role != UserRole::Broadcaster && current_user.role != UserRole::Admin {
        return Err(AppError::Forbidden(
            "only broadcasters can create stream tokens".to_string(),
        ));
    }

    let existing = load_owned_stream(&state, id, current_user.id).await?;

    // Generate raw token and its hash
    let raw_token = auth::token::generate_stream_token();
    let token_hash = auth::token::hash_token(&raw_token);
    let expires_at = Utc::now() + chrono::Duration::hours(1);

    let token_active = stream_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        stream_id: Set(id),
        token_hash: Set(token_hash),
        expires_at: Set(expires_at),
        created_at: Set(Utc::now()),
    };
    token_active.insert(&state.db).await?;

    let whip_url = format!(
        "{}/{}/whip?token={raw_token}",
        state.mediamtx_url, existing.stream_key
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

#[derive(Debug, Serialize)]
pub struct RecordingResponse {
    pub id: Uuid,
    pub stream_id: Uuid,
    pub file_path: String,
    pub duration_secs: Option<i64>,
    pub file_size_bytes: Option<i64>,
    pub created_at: chrono::DateTime<Utc>,
}

fn build_recording_response(model: recording::Model) -> RecordingResponse {
    RecordingResponse {
        id: model.id,
        stream_id: model.stream_id,
        file_path: model.file_path,
        duration_secs: model.duration_secs,
        file_size_bytes: model.file_size_bytes,
        created_at: model.created_at,
    }
}

#[derive(Debug, Deserialize)]
pub struct ListRecordingsQuery {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

/// GET /v1/streams/:id/recordings — public
async fn list_recordings(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ListRecordingsQuery>,
) -> Result<Json<PaginatedResponse<RecordingResponse>>, AppError> {
    // Verify stream exists
    let _stream = stream::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).min(100);

    let query = recording::Entity::find()
        .filter(recording::Column::StreamId.eq(id))
        .order_by_desc(recording::Column::CreatedAt);

    let total = query.clone().count(&state.db).await?;
    let total_pages = total.div_ceil(per_page);

    let models = query
        .paginate(&state.db, per_page)
        .fetch_page(page - 1)
        .await?;

    let data: Vec<RecordingResponse> = models.into_iter().map(build_recording_response).collect();

    Ok(Json(PaginatedResponse {
        data,
        pagination: Pagination {
            page,
            per_page,
            total,
            total_pages,
        },
    }))
}

pub fn stream_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/streams", post(create_stream).get(list_streams))
        .route("/v1/streams/live", get(list_live_streams))
        .route("/v1/streams/vod", get(list_vod_streams))
        .route(
            "/v1/streams/{id}",
            get(get_stream).patch(update_stream).delete(delete_stream),
        )
        .route("/v1/streams/{id}/end", post(end_stream))
        .route("/v1/streams/{id}/token", post(create_stream_token))
        .route("/v1/streams/{id}/recordings", get(list_recordings))
}
