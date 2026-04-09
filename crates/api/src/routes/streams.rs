use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use streamhub_common::{AppError, AppState};
use streamhub_entity::stream;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateStreamRequest {
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StreamResponse {
    pub id: Uuid,
    pub stream_key: String,
    pub title: Option<String>,
    pub status: stream::StreamStatus,
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

fn build_stream_response(model: stream::Model, mediamtx_base: &str) -> StreamResponse {
    let key = &model.stream_key;
    StreamResponse {
        id: model.id,
        stream_key: model.stream_key.clone(),
        title: model.title,
        status: model.status,
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

/// POST /v1/streams
async fn create_stream(
    State(state): State<AppState>,
    Json(payload): Json<CreateStreamRequest>,
) -> Result<(StatusCode, Json<DataResponse<StreamResponse>>), AppError> {
    let id = Uuid::new_v4();
    let stream_key = id.to_string();

    let active = stream::ActiveModel {
        id: Set(id),
        stream_key: Set(stream_key),
        title: Set(payload.title),
        status: Set(stream::StreamStatus::Pending),
        started_at: Set(None),
        ended_at: Set(None),
        created_at: Set(Utc::now()),
    };

    let model = active.insert(&state.db).await?;

    let resp = build_stream_response(model, &state.mediamtx_url);

    Ok((StatusCode::CREATED, Json(DataResponse { data: resp })))
}

/// GET /v1/streams/:id
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

pub fn stream_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/streams", post(create_stream))
        .route("/v1/streams/{id}", get(get_stream))
}
