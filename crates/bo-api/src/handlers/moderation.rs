//! Admin moderation handlers: cross-stream bans + chat viewer.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{Duration, Utc};
use error::AppError;
use mediamtx::keys;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::extractors::AdminUser;
use crate::state::BoAppState;

// ── Request / Response types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct BansQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
}

fn default_page() -> u64 {
    1
}
fn default_per_page() -> u64 {
    20
}

#[derive(Debug, Serialize)]
pub struct BanEntry {
    pub stream_id: Uuid,
    pub stream_title: Option<String>,
    pub user_id: String,
    pub user_email: Option<String>,
    pub is_permanent: bool,
}

#[derive(Debug, Serialize)]
pub struct BansResponse {
    pub data: BansData,
}

#[derive(Debug, Serialize)]
pub struct BansData {
    pub bans: Vec<BanEntry>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub user_id: String,
    pub display_name: String,
    pub content: String,
    pub ts: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub data: ChatData,
}

#[derive(Debug, Serialize)]
pub struct ChatData {
    pub messages: Vec<ChatMessage>,
}

// ── Handlers ───────────────────────────────────────────────────────

/// `GET /v1/admin/moderation/bans` — cross-stream banned users from recent streams.
pub async fn list_bans(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Query(query): Query<BansQuery>,
) -> Result<Json<BansResponse>, AppError> {
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);
    let since = Utc::now() - Duration::hours(24);

    // Get recent streams (live + ended within 24h)
    let recent_streams = state.uow.stream_repo().find_recent_streams(since).await?;

    // Aggregate bans from all recent streams
    let mut all_bans = Vec::new();
    for stream in &recent_streams {
        let ban_key = keys::chat_bans_set(&stream.id);
        let members = state.cache.smembers(&ban_key).await.unwrap_or_default();

        for user_id_str in members {
            let Ok(uid) = user_id_str.parse::<Uuid>() else {
                continue;
            };

            let individual_key = keys::chat_ban(&stream.id, &uid);
            let ttl = state.cache.ttl(&individual_key).await.unwrap_or(-2);

            if ttl == -2 {
                // Ban key expired — lazy cleanup: remove from the set
                let _ = state.cache.srem(&ban_key, &user_id_str).await;
                continue;
            }

            let is_permanent = ttl == -1;

            let user_email = state
                .uow
                .user_repo()
                .find_by_id(uid)
                .await
                .ok()
                .flatten()
                .map(|u| u.email);

            all_bans.push(BanEntry {
                stream_id: stream.id,
                stream_title: stream.title.clone(),
                user_id: user_id_str,
                user_email,
                is_permanent,
            });
        }
    }

    let total = all_bans.len() as u64;
    let start = ((page - 1) * per_page) as usize;
    let bans: Vec<BanEntry> = all_bans
        .into_iter()
        .skip(start)
        .take(per_page as usize)
        .collect();

    Ok(Json(BansResponse {
        data: BansData {
            bans,
            total,
            page,
            per_page,
        },
    }))
}

/// `GET /v1/admin/moderation/streams/:id/chat` — view chat history for a stream.
pub async fn stream_chat(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Path(stream_id): Path<Uuid>,
) -> Result<Json<ChatResponse>, AppError> {
    // Verify stream exists
    state
        .uow
        .stream_repo()
        .find_by_id(stream_id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    let chat_key = keys::chat_stream(&stream_id);
    let entries = state
        .cache
        .xrevrange(&chat_key, 100)
        .await
        .unwrap_or_default();

    let mut messages = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut user_id = String::new();
        let mut display_name = String::new();
        let mut content = String::new();
        let mut msg_id = String::new();

        for (field, value) in &entry.fields {
            match field.as_str() {
                "user_id" => user_id = value.clone(),
                "display_name" => display_name = value.clone(),
                "content" => content = value.clone(),
                "msg_id" => msg_id = value.clone(),
                _ => {}
            }
        }

        messages.push(ChatMessage {
            id: if msg_id.is_empty() {
                entry.id.clone()
            } else {
                msg_id
            },
            user_id,
            display_name,
            content,
            ts: entry.id.clone(),
        });
    }

    Ok(Json(ChatResponse {
        data: ChatData { messages },
    }))
}
