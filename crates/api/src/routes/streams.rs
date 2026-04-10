use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use common::{AppError, AppState};
use entity::stream;
use entity::stream_token;
use entity::user::UserRole;
use sea_orm::Set;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    let models = state.uow.stream_repo().list_vod().await?;

    let data: Vec<LiveStreamResponse> = models
        .into_iter()
        .map(|m| build_live_stream_response(m, &state.config.mediamtx_url))
        .collect();

    Ok(Json(DataResponse { data }))
}

/// GET /v1/streams/live — public, returns all live streams
async fn list_live_streams(
    State(state): State<AppState>,
) -> Result<Json<DataResponse<Vec<LiveStreamResponse>>>, AppError> {
    let models = state.uow.stream_repo().list_live().await?;

    let data: Vec<LiveStreamResponse> = models
        .into_iter()
        .map(|m| build_live_stream_response(m, &state.config.mediamtx_url))
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

    let txn = state.uow.begin().await?;
    let model = txn.stream_repo().create(active).await?;
    txn.commit().await?;

    let resp = build_stream_response(model, &state.config.mediamtx_url);

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

    let data: Vec<StreamResponse> = result
        .items
        .into_iter()
        .map(|m| build_stream_response(m, &state.config.mediamtx_url))
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

/// GET /v1/streams/:id — public
async fn get_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamResponse>>, AppError> {
    let model = state
        .uow
        .stream_repo()
        .find_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let resp = build_stream_response(model, &state.config.mediamtx_url);
    Ok(Json(DataResponse { data: resp }))
}

/// PATCH /v1/streams/:id — owner only
async fn update_stream(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateStreamRequest>,
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

    let resp = build_stream_response(model, &state.config.mediamtx_url);
    Ok(Json(DataResponse { data: resp }))
}

/// DELETE /v1/streams/:id — owner only, not allowed if Live
async fn delete_stream(
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

/// POST /v1/streams/:id/end — owner only, must be Live
async fn end_stream(
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

    let resp = build_stream_response(model, &state.config.mediamtx_url);
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

    let txn = state.uow.begin().await?;

    let existing = txn
        .stream_repo()
        .find_by_id_for_update(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if existing.user_id != Some(current_user.id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

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
    txn.stream_token_repo().create(token_active).await?;
    txn.commit().await?;

    let whip_url = format!(
        "{}/{}/whip?token={raw_token}",
        state.config.mediamtx_url, existing.stream_key
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use common::AppConfig;
    use entity::user;
    use http_body_util::BodyExt;
    use repo::UnitOfWork;
    use sea_orm::{DbBackend, MockDatabase, MockExecResult};
    use tower::ServiceExt;

    const JWT_SECRET: &str = "test-secret";

    fn test_config() -> AppConfig {
        AppConfig {
            database_url: String::new(),
            host: "127.0.0.1".to_string(),
            port: 0,
            mediamtx_url: "http://localhost:9997".to_string(),
            jwt_secret: JWT_SECRET.to_string(),
            recordings_path: "/tmp/recordings".to_string(),
        }
    }

    fn broadcaster_user() -> user::Model {
        user::Model {
            id: Uuid::new_v4(),
            email: "broadcaster@example.com".to_string(),
            password_hash: String::new(),
            role: user::UserRole::Broadcaster,
            created_at: Utc::now(),
        }
    }

    fn viewer_user() -> user::Model {
        user::Model {
            id: Uuid::new_v4(),
            email: "viewer@example.com".to_string(),
            password_hash: String::new(),
            role: user::UserRole::Viewer,
            created_at: Utc::now(),
        }
    }

    fn test_stream(user_id: Uuid) -> stream::Model {
        let id = Uuid::new_v4();
        stream::Model {
            id,
            user_id: Some(user_id),
            stream_key: id.to_string(),
            title: Some("Test Stream".to_string()),
            status: stream::StreamStatus::Pending,
            vod_status: stream::VodStatus::None,
            started_at: None,
            ended_at: None,
            created_at: Utc::now(),
            hls_url: None,
        }
    }

    fn auth_header(user_id: Uuid) -> String {
        let token = auth::jwt::sign_access_token(user_id, JWT_SECRET).unwrap();
        format!("Bearer {token}")
    }

    async fn body_to_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn create_stream_success() {
        let user = broadcaster_user();
        let s = test_stream(user.id);

        // Mock: auth middleware find_by_id, then txn create
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user.clone()]]) // auth: find_by_id
            .append_query_results([vec![s.clone()]]) // create stream
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = stream_routes().with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/streams")
            .header("content-type", "application/json")
            .header("authorization", auth_header(user.id))
            .body(Body::from(r#"{"title":"My Stream"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["data"]["title"], "Test Stream");
    }

    #[tokio::test]
    async fn create_stream_viewer_forbidden() {
        let user = viewer_user();

        // Mock: auth middleware find_by_id
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user.clone()]]) // auth: find_by_id
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = stream_routes().with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/streams")
            .header("content-type", "application/json")
            .header("authorization", auth_header(user.id))
            .body(Body::from(r#"{"title":"My Stream"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
