//! Admin stream management handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::Utc;
use entity::stream::StreamStatus;
use error::AppError;
use mediamtx::keys;
use sea_orm::Set;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::extractors::AdminUser;
use crate::state::BoAppState;

// ── Request / Response types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListStreamsQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
    pub status: Option<StreamStatus>,
    pub q: Option<String>,
}

fn default_page() -> u64 {
    1
}
fn default_per_page() -> u64 {
    20
}

#[derive(Debug, Serialize)]
pub struct StreamListItem {
    pub id: Uuid,
    pub title: Option<String>,
    pub stream_key: String,
    pub status: StreamStatus,
    pub vod_status: entity::stream::VodStatus,
    pub owner_email: Option<String>,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub viewer_count: u32,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListStreamsResponse {
    pub data: ListStreamsData,
}

#[derive(Debug, Serialize)]
pub struct ListStreamsData {
    pub streams: Vec<StreamListItem>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Debug, Serialize)]
pub struct StreamDetail {
    pub id: Uuid,
    pub title: Option<String>,
    pub stream_key: String,
    pub status: StreamStatus,
    pub vod_status: entity::stream::VodStatus,
    pub owner_id: Option<Uuid>,
    pub owner_email: Option<String>,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub ended_at: Option<chrono::DateTime<Utc>>,
    pub hls_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub viewer_count: u32,
    pub created_at: chrono::DateTime<Utc>,
    pub active_session: Option<String>,
    pub mtx_instance: Option<String>,
    pub chat_message_count: u64,
}

#[derive(Debug, Serialize)]
pub struct DataResponse<T: Serialize> {
    pub data: T,
}

// ── Handlers ───────────────────────────────────────────────────────

/// `GET /v1/admin/streams` — list streams with search / filter / pagination.
pub async fn list_streams(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Query(query): Query<ListStreamsQuery>,
) -> Result<Json<ListStreamsResponse>, AppError> {
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);

    let result = state
        .uow
        .stream_repo()
        .find_streams_paginated(page, per_page, query.status, query.q.as_deref())
        .await?;

    let user_repo = state.uow.user_repo();
    let mut streams = Vec::with_capacity(result.items.len());

    for s in &result.items {
        let owner_email = if let Some(uid) = s.user_id {
            user_repo
                .find_by_id(uid)
                .await
                .map_err(AppError::from)?
                .map(|u| u.email)
        } else {
            None
        };

        let viewer_count = state
            .cache
            .get(&keys::viewer_count(&s.id))
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);

        streams.push(StreamListItem {
            id: s.id,
            title: s.title.clone(),
            stream_key: s.stream_key.clone(),
            status: s.status.clone(),
            vod_status: s.vod_status.clone(),
            owner_email,
            started_at: s.started_at,
            ended_at: s.ended_at,
            viewer_count,
            created_at: s.created_at,
        });
    }

    Ok(Json(ListStreamsResponse {
        data: ListStreamsData {
            streams,
            total: result.total,
            page,
            per_page,
        },
    }))
}

/// `GET /v1/admin/streams/:id` — stream detail with Redis enrichment.
pub async fn stream_detail(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Path(stream_id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamDetail>>, AppError> {
    let stream = state
        .uow
        .stream_repo()
        .find_by_id(stream_id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let owner_email = if let Some(uid) = stream.user_id {
        state
            .uow
            .user_repo()
            .find_by_id(uid)
            .await
            .map_err(AppError::from)?
            .map(|u| u.email)
    } else {
        None
    };

    let viewer_count = state
        .cache
        .get(&keys::viewer_count(&stream.id))
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    let active_session = state
        .cache
        .get(&keys::stream_active_session(&stream.id))
        .await
        .ok()
        .flatten();

    let mtx_instance = if let Some(ref sid_str) = active_session {
        if let Ok(sid) = sid_str.parse::<Uuid>() {
            state
                .cache
                .get(&keys::session_mtx(&sid))
                .await
                .ok()
                .flatten()
        } else {
            None
        }
    } else {
        None
    };

    let chat_message_count = state
        .cache
        .xlen(&keys::chat_stream(&stream.id))
        .await
        .unwrap_or(0);

    Ok(Json(DataResponse {
        data: StreamDetail {
            id: stream.id,
            title: stream.title,
            stream_key: stream.stream_key,
            status: stream.status,
            vod_status: stream.vod_status,
            owner_id: stream.user_id,
            owner_email,
            started_at: stream.started_at,
            ended_at: stream.ended_at,
            hls_url: stream.hls_url,
            thumbnail_url: stream.thumbnail_url,
            viewer_count,
            created_at: stream.created_at,
            active_session,
            mtx_instance,
            chat_message_count,
        },
    }))
}

/// `POST /v1/admin/streams/:id/end` — force-end a live stream.
///
/// Synchronous: updates DB status to Ended. Publishes an event for the api
/// redis_subscriber to handle async cleanup (session keys, thumbnail, etc.).
pub async fn force_end(
    admin: AdminUser,
    State(state): State<BoAppState>,
    Path(stream_id): Path<Uuid>,
) -> Result<Json<DataResponse<StreamDetail>>, AppError> {
    let stream = state
        .uow
        .stream_repo()
        .find_by_id(stream_id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if stream.status != StreamStatus::Live {
        return Err(AppError::Conflict("Stream is not live".to_string()));
    }

    // DB update: status = ended, ended_at = now
    let mut active_model: entity::stream::ActiveModel = stream.into();
    active_model.status = Set(StreamStatus::Ended);
    active_model.ended_at = Set(Some(Utc::now()));
    let updated = state.uow.stream_repo().update(active_model).await?;

    // Block future `/token` issuance for this stream so the broadcaster
    // frontend cannot auto-reconnect after the MTX session is kicked.
    if let Err(e) = state
        .cache
        .set(&keys::stream_force_ended(&stream_id), "1", None)
        .await
    {
        tracing::error!(error = %e, %stream_id, "Failed to persist force-end flag");
    }

    // Publish admin_force_end event for api's async cleanup
    let event = serde_json::json!({
        "stream_id": stream_id,
        "requested_by": admin.0.id,
        "requested_at": Utc::now().to_rfc3339(),
    });
    if let Err(e) = state
        .pubsub
        .publish(keys::ADMIN_FORCE_END_CHANNEL, &event.to_string())
        .await
    {
        tracing::error!(error = %e, "Failed to publish admin_force_end event");
    }

    // Build response with Redis enrichment
    let owner_email = if let Some(uid) = updated.user_id {
        state
            .uow
            .user_repo()
            .find_by_id(uid)
            .await
            .map_err(AppError::from)?
            .map(|u| u.email)
    } else {
        None
    };

    Ok(Json(DataResponse {
        data: StreamDetail {
            id: updated.id,
            title: updated.title,
            stream_key: updated.stream_key,
            status: updated.status,
            vod_status: updated.vod_status,
            owner_id: updated.user_id,
            owner_email,
            started_at: updated.started_at,
            ended_at: updated.ended_at,
            hls_url: updated.hls_url,
            thumbnail_url: updated.thumbnail_url,
            viewer_count: 0,
            created_at: updated.created_at,
            active_session: None,
            mtx_instance: None,
            chat_message_count: 0,
        },
    }))
}
