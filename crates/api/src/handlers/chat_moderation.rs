use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::{AppError, AppState};
use entity::user::UserRole;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::extractors::AppJson;
use crate::handlers::streams::DataResponse;
use crate::middleware::CurrentUser;
use crate::ws::types::ServerMessage;

fn assert_can_moderate(
    current_user: &CurrentUser,
    stream_owner_id: Option<Uuid>,
) -> Result<(), AppError> {
    if current_user.role == UserRole::Admin {
        return Ok(());
    }
    if stream_owner_id == Some(current_user.id) {
        return Ok(());
    }
    Err(AppError::Forbidden(
        "not the stream owner or admin".to_string(),
    ))
}

async fn load_stream_owner(state: &AppState, stream_id: Uuid) -> Result<Option<Uuid>, AppError> {
    let model = state
        .uow
        .stream_repo()
        .find_by_id(stream_id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;
    Ok(model.user_id)
}

/// `DELETE /v1/streams/:stream_id/chat/messages/:msg_id`
#[tracing::instrument(skip(state), fields(%stream_id, %msg_id, user_id = %current_user.id))]
pub(crate) async fn delete_message_handler(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path((stream_id, msg_id)): Path<(Uuid, String)>,
) -> Result<StatusCode, AppError> {
    let owner = load_stream_owner(&state, stream_id).await?;
    assert_can_moderate(&current_user, owner)?;

    let cache = state.cache.as_ref();
    let msgindex_key = mediamtx::keys::chat_msgindex(&stream_id);
    let entry_id = cache
        .hget(&msgindex_key, &msg_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("MESSAGE_NOT_FOUND".to_string()))?;

    let stream_key = mediamtx::keys::chat_stream(&stream_id);
    let deleted = cache
        .xdel(&stream_key, &entry_id)
        .await
        .map_err(|e| AppError::Internal(format!("XDEL failed: {e}")))?;

    if deleted == 0 {
        // Stale index: entry was already gone (previous delete succeeded but
        // HDEL failed). Clean up the orphan index entry and return 404.
        if let Err(e) = cache.hdel(&msgindex_key, &msg_id).await {
            tracing::warn!(error = %e, "HDEL stale msgindex cleanup failed");
        }
        return Err(AppError::NotFound("MESSAGE_ALREADY_DELETED".to_string()));
    }

    if let Err(e) = cache.hdel(&msgindex_key, &msg_id).await {
        tracing::warn!(error = %e, "HDEL msgindex failed");
    }

    let envelope = ServerMessage::ChatMessageDeleted {
        stream_id,
        msg_id: msg_id.clone(),
    };
    let payload =
        serde_json::to_string(&envelope).map_err(|e| AppError::Internal(e.to_string()))?;
    let channel = mediamtx::keys::chat_pubsub_channel(&stream_id);
    state
        .pubsub
        .publish(&channel, &payload)
        .await
        .map_err(|e| AppError::Internal(format!("chat delete PUBLISH failed: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub(crate) struct BanUserRequest {
    pub user_id: Uuid,
    pub duration_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BanUserResponse {
    pub user_id: Uuid,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

/// `POST /v1/streams/:stream_id/chat/bans`
#[tracing::instrument(skip(state, body), fields(%stream_id, user_id = %current_user.id))]
pub(crate) async fn ban_user_handler(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
    AppJson(body): AppJson<BanUserRequest>,
) -> Result<(StatusCode, Json<DataResponse<BanUserResponse>>), AppError> {
    let owner = load_stream_owner(&state, stream_id).await?;
    assert_can_moderate(&current_user, owner)?;

    if body.user_id == current_user.id {
        return Err(AppError::BadRequest("cannot ban yourself".to_string()));
    }

    if let Some(d) = body.duration_secs {
        if d == 0 {
            return Err(AppError::BadRequest(
                "duration_secs must be positive or null".to_string(),
            ));
        }
    }

    let cache = state.cache.as_ref();
    let ban_key = mediamtx::keys::chat_ban(&stream_id, &body.user_id);
    let bans_set_key = mediamtx::keys::chat_bans_set(&stream_id);

    cache
        .set(&ban_key, "1", body.duration_secs)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let _ = cache.sadd(&bans_set_key, &body.user_id.to_string()).await;

    let expires_at = body
        .duration_secs
        .map(|d| Utc::now() + chrono::Duration::seconds(d as i64));

    Ok((
        StatusCode::CREATED,
        Json(DataResponse {
            data: BanUserResponse {
                user_id: body.user_id,
                expires_at,
            },
        }),
    ))
}

#[derive(Debug, Serialize)]
pub(crate) struct BannedUserEntry {
    pub user_id: Uuid,
    pub display_name: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

/// `GET /v1/streams/:stream_id/chat/bans`
#[tracing::instrument(skip(state), fields(%stream_id, user_id = %current_user.id))]
pub(crate) async fn list_bans_handler(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(stream_id): Path<Uuid>,
) -> Result<Json<DataResponse<Vec<BannedUserEntry>>>, AppError> {
    let owner = load_stream_owner(&state, stream_id).await?;
    assert_can_moderate(&current_user, owner)?;

    let cache = state.cache.as_ref();
    let bans_set_key = mediamtx::keys::chat_bans_set(&stream_id);
    let members = cache
        .smembers(&bans_set_key)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut result = Vec::new();
    for uid_str in members {
        let uid = match Uuid::parse_str(&uid_str) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let ban_key = mediamtx::keys::chat_ban(&stream_id, &uid);
        let ttl = cache
            .ttl(&ban_key)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if ttl == -2 {
            let _ = cache.srem(&bans_set_key, &uid_str).await;
            continue;
        }

        let expires_at = if ttl == -1 {
            None
        } else {
            Some(Utc::now() + chrono::Duration::seconds(ttl))
        };

        let display_name = state
            .uow
            .user_repo()
            .find_by_id(uid)
            .await
            .ok()
            .flatten()
            .map(|u| {
                u.email
                    .split_once('@')
                    .map(|(l, _)| l.to_string())
                    .unwrap_or(u.email)
            });

        result.push(BannedUserEntry {
            user_id: uid,
            display_name,
            expires_at,
        });
    }

    Ok(Json(DataResponse { data: result }))
}

/// `DELETE /v1/streams/:stream_id/chat/bans/:user_id`
#[tracing::instrument(skip(state), fields(%stream_id, %user_id, caller_id = %current_user.id))]
pub(crate) async fn unban_user_handler(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path((stream_id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let owner = load_stream_owner(&state, stream_id).await?;
    assert_can_moderate(&current_user, owner)?;

    let cache = state.cache.as_ref();
    let ban_key = mediamtx::keys::chat_ban(&stream_id, &user_id);
    let bans_set_key = mediamtx::keys::chat_bans_set(&stream_id);

    let deleted_key = cache
        .get(&ban_key)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let removed_set = cache
        .srem(&bans_set_key, &user_id.to_string())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if deleted_key.is_none() && !removed_set {
        return Err(AppError::NotFound("BAN_NOT_FOUND".to_string()));
    }

    let _ = cache.del(&ban_key).await;

    Ok(StatusCode::NO_CONTENT)
}
