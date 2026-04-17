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
    pub broadcaster_id: Uuid,
    pub broadcaster_email: Option<String>,
    /// A representative stream_id owned by this broadcaster, so the admin
    /// frontend can call `DELETE /v1/streams/:stream_id/chat/bans/:user_id`.
    pub stream_id: Uuid,
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

/// `GET /v1/admin/moderation/bans` — per-broadcaster banned users aggregated
/// from recent streams' owners.
#[tracing::instrument(
    skip(state, _admin),
    fields(admin_id = %_admin.0.id, page = query.page, per_page = query.per_page)
)]
pub async fn list_bans(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Query(query): Query<BansQuery>,
) -> Result<Json<BansResponse>, AppError> {
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);
    let since = Utc::now() - Duration::hours(24);

    let broadcaster_stream = list_bans_scan_broadcasters(&state, since).await?;
    let mut broadcaster_ids: Vec<Uuid> = broadcaster_stream.keys().copied().collect();
    broadcaster_ids.sort();

    let mut all_bans = list_bans_resolve(&state, &broadcaster_ids, &broadcaster_stream).await;
    let (total, bans) = list_bans_aggregate(&mut all_bans, page, per_page);

    Ok(Json(BansResponse {
        data: BansData {
            bans,
            total,
            page,
            per_page,
        },
    }))
}

/// Reads recent streams from Postgres and groups them by broadcaster, keeping
/// a representative stream_id per broadcaster so the admin frontend can hit
/// `DELETE /v1/streams/:stream_id/chat/bans/:user_id`.
#[tracing::instrument(skip(state), fields(since = %since))]
async fn list_bans_scan_broadcasters(
    state: &BoAppState,
    since: chrono::DateTime<Utc>,
) -> Result<std::collections::HashMap<Uuid, Uuid>, AppError> {
    let recent_streams = state.uow.stream_repo().find_recent_streams(since).await?;
    let mut broadcaster_stream: std::collections::HashMap<Uuid, Uuid> =
        std::collections::HashMap::new();
    for s in &recent_streams {
        if let Some(uid) = s.user_id {
            broadcaster_stream.entry(uid).or_insert(s.id);
        }
    }
    Ok(broadcaster_stream)
}

/// For every known broadcaster, reads their chat ban set from Redis, prunes
/// stale members (TTL == -2), and resolves banned user emails from Postgres.
/// This is the N*M hot path — keep the surrounding span name stable for
/// dashboards.
#[tracing::instrument(
    skip(state, broadcaster_ids, broadcaster_stream),
    fields(broadcaster_count = broadcaster_ids.len())
)]
async fn list_bans_resolve(
    state: &BoAppState,
    broadcaster_ids: &[Uuid],
    broadcaster_stream: &std::collections::HashMap<Uuid, Uuid>,
) -> Vec<BanEntry> {
    let mut all_bans = Vec::new();
    for broadcaster_id in broadcaster_ids {
        let bans_set_key = keys::chat_bans_set(broadcaster_id);
        let members = state
            .cache
            .smembers(&bans_set_key)
            .await
            .unwrap_or_default();

        let broadcaster_email = state
            .uow
            .user_repo()
            .find_by_id(*broadcaster_id)
            .await
            .ok()
            .flatten()
            .map(|u| u.email);

        for user_id_str in members {
            let Ok(uid) = user_id_str.parse::<Uuid>() else {
                continue;
            };

            let individual_key = keys::chat_ban(broadcaster_id, &uid);
            let ttl = state.cache.ttl(&individual_key).await.unwrap_or(-2);

            if ttl == -2 {
                let _ = state.cache.srem(&bans_set_key, &user_id_str).await;
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
                broadcaster_id: *broadcaster_id,
                broadcaster_email: broadcaster_email.clone(),
                stream_id: broadcaster_stream[broadcaster_id],
                user_id: user_id_str,
                user_email,
                is_permanent,
            });
        }
    }
    all_bans
}

/// Sorts for stable pagination and slices to the requested page.
#[tracing::instrument(skip(all_bans), fields(total_before = all_bans.len(), page, per_page))]
fn list_bans_aggregate(
    all_bans: &mut Vec<BanEntry>,
    page: u64,
    per_page: u64,
) -> (u64, Vec<BanEntry>) {
    all_bans.sort_by(|a, b| {
        a.broadcaster_id
            .cmp(&b.broadcaster_id)
            .then_with(|| a.user_id.cmp(&b.user_id))
    });

    let total = all_bans.len() as u64;
    let start = ((page - 1) * per_page) as usize;
    let bans: Vec<BanEntry> = all_bans
        .drain(..)
        .skip(start)
        .take(per_page as usize)
        .collect();
    (total, bans)
}

/// `GET /v1/admin/moderation/streams/:id/chat` — view chat history for a stream.
#[tracing::instrument(
    skip(state, _admin),
    fields(admin_id = %_admin.0.id, stream_id = %stream_id)
)]
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
