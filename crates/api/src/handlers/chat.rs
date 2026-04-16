//! Live-chat WebSocket action handlers backed by Redis Streams + pub/sub.
//!
//! Messages are only persisted in Redis (no Postgres). The canonical message
//! flow is: `send_chat` → XADD (with 24h TTL) → PUBLISH → every API instance's
//! Redis subscriber task picks up the payload and fans it out to local WS
//! connections that have called `subscribe_chat` for that room. The publisher
//! instance does NOT fan out locally on its own to avoid double delivery — the
//! pub/sub loopback is the single source of truth.

use std::sync::Arc;

use cache::{CacheStore, PubSub};
use uuid::Uuid;

use crate::ws::manager::WsManager;
use crate::ws::types::{ChatErrorReason, ChatMessagePayload, ServerMessage};

/// Hard limit on chat message size (characters, not bytes).
pub const CHAT_MAX_CHARS: usize = 500;
/// Per-stream approximate trim — Redis XADD MAXLEN `~`.
pub const CHAT_STREAM_MAXLEN: usize = 1000;
/// TTL applied to the chat stream key after every XADD.
pub const CHAT_STREAM_TTL_SECS: u64 = 86_400;
/// Scrollback size pushed on subscribe.
pub const CHAT_HISTORY_LIMIT: usize = 50;

/// Identity supplied to `send_chat`. `None` means the WS is unauthenticated.
#[derive(Debug, Clone)]
pub struct ChatUser {
    /// Authenticated user id.
    pub id: Uuid,
    /// Email used to derive the display name (local-part before `@`).
    pub email: String,
}

impl ChatUser {
    /// Display name = email local-part (stable, already validated at register).
    pub fn display_name(&self) -> String {
        self.email
            .split_once('@')
            .map(|(local, _)| local.to_string())
            .unwrap_or_else(|| self.email.clone())
    }
}

/// Outcome of a `send_chat` call — lets the caller reply with `ChatError` on
/// failure without bubbling an `anyhow::Error` through the WS loop.
#[derive(Debug)]
pub enum SendChatOutcome {
    /// Message accepted and published.
    Accepted,
    /// Rejected for a user-visible reason.
    Rejected(ChatErrorReason),
}

/// Handle a `send_chat` action. Performs: length check → per-user rate-limit
/// lock → active-session check → XADD → PUBLISH.
///
/// The WS payload is excluded from tracing to avoid leaking chat contents.
#[tracing::instrument(
    name = "chat.send",
    skip(cache, pubsub, content),
    fields(user_id = ?user.as_ref().map(|u| u.id), %stream_id)
)]
pub async fn handle_send_chat(
    cache: &dyn CacheStore,
    pubsub: &dyn PubSub,
    uow: &repo::UnitOfWork,
    user: Option<&ChatUser>,
    stream_id: Uuid,
    content: String,
) -> SendChatOutcome {
    let Some(user) = user else {
        return SendChatOutcome::Rejected(ChatErrorReason::Unauthorized);
    };

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return SendChatOutcome::Rejected(ChatErrorReason::TooLong);
    }
    if trimmed.chars().count() > CHAT_MAX_CHARS {
        return SendChatOutcome::Rejected(ChatErrorReason::TooLong);
    }

    let lock_key = mediamtx::keys::chat_ratelimit(&user.id);
    match cache.set_nx(&lock_key, "1", Some(1)).await {
        Ok(true) => {}
        Ok(false) => return SendChatOutcome::Rejected(ChatErrorReason::RateLimited),
        Err(e) => {
            tracing::warn!(error = %e, "chat rate-limit set_nx failed");
            return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
        }
    }

    // Resolve stream owner for per-broadcaster ban check.
    // Try Redis first, fallback to DB if missing or malformed.
    let cached_owner = match cache.get(&mediamtx::keys::stream_owner(&stream_id)).await {
        Ok(Some(s)) => Uuid::parse_str(&s).ok(),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "stream owner lookup failed");
            return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
        }
    };

    let owner_id = match cached_owner {
        Some(uid) => Some(uid),
        None => {
            // Redis miss or malformed — fallback to DB and rehydrate cache.
            match uow.stream_repo().find_by_id(stream_id).await {
                Ok(Some(stream)) => {
                    if let Some(uid) = stream.user_id {
                        let _ = cache
                            .set(
                                &mediamtx::keys::stream_owner(&stream_id),
                                &uid.to_string(),
                                None,
                            )
                            .await;
                        Some(uid)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
    };

    if let Some(owner_id) = owner_id {
        let ban_key = mediamtx::keys::chat_ban(&owner_id, &user.id);
        match cache.get(&ban_key).await {
            Ok(Some(_)) => return SendChatOutcome::Rejected(ChatErrorReason::Banned),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "chat ban check failed");
                return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
            }
        }
    }

    let active_key = mediamtx::keys::stream_active_session(&stream_id);
    match cache.get(&active_key).await {
        Ok(Some(_)) => {}
        Ok(None) => return SendChatOutcome::Rejected(ChatErrorReason::UnknownStream),
        Err(e) => {
            tracing::warn!(error = %e, "chat active-session lookup failed");
            return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
        }
    }

    let display_name = user.display_name();
    let user_id_str = user.id.to_string();
    let msg_id = Uuid::now_v7();
    let msg_id_str = msg_id.to_string();
    let stream_key = mediamtx::keys::chat_stream(&stream_id);

    let entry_id = match cache
        .xadd_maxlen(
            &stream_key,
            CHAT_STREAM_MAXLEN,
            &[
                ("id", msg_id_str.as_str()),
                ("user_id", user_id_str.as_str()),
                ("display_name", display_name.as_str()),
                ("content", trimmed),
            ],
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "chat XADD failed");
            return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
        }
    };

    if let Err(e) = cache.expire(&stream_key, CHAT_STREAM_TTL_SECS).await {
        tracing::warn!(error = %e, "chat EXPIRE failed (message still stored)");
    }

    let msgindex_key = mediamtx::keys::chat_msgindex(&stream_id);
    if let Err(e) = cache.hset(&msgindex_key, &msg_id_str, &entry_id).await {
        tracing::warn!(error = %e, "chat HSET msgindex failed");
    }
    if let Err(e) = cache.expire(&msgindex_key, CHAT_STREAM_TTL_SECS).await {
        tracing::warn!(error = %e, "chat msgindex EXPIRE failed");
    }

    let message = ChatMessagePayload {
        id: msg_id_str,
        stream_id,
        user_id: user.id,
        display_name,
        content: trimmed.to_string(),
        ts_ms: parse_entry_ts_ms(&entry_id),
    };

    let envelope = ServerMessage::ChatMessage { stream_id, message };
    let payload = match serde_json::to_string(&envelope) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "chat serialize failed");
            return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
        }
    };

    let channel = mediamtx::keys::chat_pubsub_channel(&stream_id);
    if let Err(e) = pubsub.publish(&channel, &payload).await {
        tracing::warn!(error = %e, "chat PUBLISH failed");
        return SendChatOutcome::Rejected(ChatErrorReason::Unknown);
    }

    SendChatOutcome::Accepted
}

/// Handle a `subscribe_chat` action: register the connection as a chat room
/// subscriber and immediately push the most recent history.
#[tracing::instrument(name = "chat.subscribe", skip(cache, ws_manager), fields(%stream_id))]
pub async fn handle_subscribe_chat(
    cache: &dyn CacheStore,
    ws_manager: &WsManager,
    conn_id: Uuid,
    stream_id: Uuid,
) {
    ws_manager.subscribe_chat(conn_id, stream_id).await;

    let key = mediamtx::keys::chat_stream(&stream_id);
    let entries = match cache.xrevrange(&key, CHAT_HISTORY_LIMIT).await {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(error = %err, "chat XREVRANGE failed");
            return;
        }
    };

    // xrevrange returns newest → oldest; flip for oldest → newest delivery.
    let messages: Vec<ChatMessagePayload> = entries
        .into_iter()
        .rev()
        .map(|entry| {
            let ts_ms = parse_entry_ts_ms(&entry.id);
            let mut msg_id = String::new();
            let mut user_id = Uuid::nil();
            let mut display_name = String::new();
            let mut content = String::new();
            for (f, v) in entry.fields {
                match f.as_str() {
                    "id" => msg_id = v,
                    "user_id" => user_id = Uuid::parse_str(&v).unwrap_or(Uuid::nil()),
                    "display_name" => display_name = v,
                    "content" => content = v,
                    _ => {}
                }
            }
            if msg_id.is_empty() {
                msg_id = entry.id;
            }
            ChatMessagePayload {
                id: msg_id,
                stream_id,
                user_id,
                display_name,
                content,
                ts_ms,
            }
        })
        .collect();

    let msg = ServerMessage::ChatHistory {
        stream_id,
        messages,
    };
    ws_manager.send_to(&conn_id, &msg).await;
}

/// Handle an `unsubscribe_chat` action.
pub async fn handle_unsubscribe_chat(ws_manager: &WsManager, conn_id: Uuid, stream_id: Uuid) {
    ws_manager.unsubscribe_chat(&conn_id, &stream_id).await;
}

/// Parses the millisecond component of a Redis Stream entry id like
/// `"1712345678901-0"`. Returns 0 if parsing fails.
fn parse_entry_ts_ms(id: &str) -> i64 {
    id.split('-')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

/// Spawn the chat pub/sub subscriber for a given stream on this instance the
/// first time someone subscribes locally. Idempotent across WS connections:
/// `WsManager::try_claim_chat_pubsub` ensures only one forwarder task per
/// stream per instance, so a chat message is fanned out exactly once even when
/// many WS subscribe to the same room. The slot is released when the task
/// exits (after `pubsub.unsubscribe` closes the receiver).
pub async fn ensure_chat_pubsub_task(
    pubsub: Arc<dyn PubSub>,
    ws_manager: Arc<WsManager>,
    stream_id: Uuid,
) {
    if !ws_manager.try_claim_chat_pubsub(stream_id).await {
        // Another WS already spawned the forwarder for this room.
        return;
    }

    let channel = mediamtx::keys::chat_pubsub_channel(&stream_id);
    let mut rx = match pubsub.subscribe(&channel).await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::warn!(error = %e, channel = %channel, "chat pubsub subscribe failed");
            ws_manager.release_chat_pubsub(&stream_id).await;
            return;
        }
    };

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(payload) => {
                    ws_manager
                        .broadcast_chat_to_room(&stream_id, &payload)
                        .await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "chat subscriber lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::warn!(%stream_id, "chat pubsub channel closed");
                    break;
                }
            }
        }
        // Free the slot so a future stream session on the same id can spawn
        // a fresh forwarder.
        ws_manager.release_chat_pubsub(&stream_id).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cache::{InMemoryCache, InMemoryPubSub};
    use sea_orm::{DbBackend, MockDatabase};

    fn mock_uow() -> repo::UnitOfWork {
        repo::UnitOfWork::new(MockDatabase::new(DbBackend::Postgres).into_connection())
    }

    fn user(id: Uuid, email: &str) -> ChatUser {
        ChatUser {
            id,
            email: email.to_string(),
        }
    }

    async fn seed_active_stream(cache: &InMemoryCache, stream_id: &Uuid) {
        cache
            .set(
                &mediamtx::keys::stream_active_session(stream_id),
                &Uuid::new_v4().to_string(),
                None,
            )
            .await
            .unwrap();
    }

    async fn seed_stream_owner(cache: &InMemoryCache, stream_id: &Uuid, owner_id: &Uuid) {
        cache
            .set(
                &mediamtx::keys::stream_owner(stream_id),
                &owner_id.to_string(),
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_when_unauthenticated() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            None,
            Uuid::new_v4(),
            "hi".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn rejects_over_500_chars() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        let content = "a".repeat(501);
        let out =
            handle_send_chat(&cache, &pubsub, &mock_uow(), Some(&u), stream_id, content).await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::TooLong)
        ));
    }

    #[tokio::test]
    async fn rejects_empty_content() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "   ".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::TooLong)
        ));
    }

    #[tokio::test]
    async fn rejects_unknown_stream() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let u = user(Uuid::new_v4(), "alice@example.com");
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            Uuid::new_v4(),
            "hi".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::UnknownStream)
        ));
    }

    #[tokio::test]
    async fn second_send_within_a_second_is_rate_limited() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        let a = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "hi".into(),
        )
        .await;
        assert!(matches!(a, SendChatOutcome::Accepted));
        let b = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "hi again".into(),
        )
        .await;
        assert!(matches!(
            b,
            SendChatOutcome::Rejected(ChatErrorReason::RateLimited)
        ));
    }

    #[tokio::test]
    async fn accepted_write_lands_in_history() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "hello".into(),
        )
        .await;
        assert!(matches!(out, SendChatOutcome::Accepted));
        let key = mediamtx::keys::chat_stream(&stream_id);
        let entries = cache.xrevrange(&key, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        let content = entries[0]
            .fields
            .iter()
            .find(|(f, _)| f == "content")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn display_name_is_email_local_part() {
        let u = user(Uuid::new_v4(), "alice@example.com");
        assert_eq!(u.display_name(), "alice");
    }

    #[tokio::test]
    async fn banned_user_cannot_send() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        seed_stream_owner(&cache, &stream_id, &owner_id).await;
        let u = user(Uuid::new_v4(), "banned@example.com");
        let ban_key = mediamtx::keys::chat_ban(&owner_id, &u.id);
        cache.set(&ban_key, "1", None).await.unwrap();
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "hi".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::Banned)
        ));
    }

    #[tokio::test]
    async fn msg_id_is_uuid_v7_format() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "hello".into(),
        )
        .await;
        assert!(matches!(out, SendChatOutcome::Accepted));

        let key = mediamtx::keys::chat_stream(&stream_id);
        let entries = cache.xrevrange(&key, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        let id_field = entries[0]
            .fields
            .iter()
            .find(|(f, _)| f == "id")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(Uuid::parse_str(id_field).is_ok());
    }

    #[tokio::test]
    async fn msgindex_written_on_send() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "hi".into(),
        )
        .await;

        let key = mediamtx::keys::chat_stream(&stream_id);
        let entries = cache.xrevrange(&key, 10).await.unwrap();
        let msg_id = entries[0]
            .fields
            .iter()
            .find(|(f, _)| f == "id")
            .map(|(_, v)| v.clone())
            .unwrap();
        let entry_id = &entries[0].id;

        let msgindex_key = mediamtx::keys::chat_msgindex(&stream_id);
        let stored = cache.hget(&msgindex_key, &msg_id).await.unwrap();
        assert_eq!(stored.as_deref(), Some(entry_id.as_str()));
    }

    #[tokio::test]
    async fn xdel_removes_message_from_stream() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let stream_id = Uuid::new_v4();
        seed_active_stream(&cache, &stream_id).await;
        let u = user(Uuid::new_v4(), "alice@example.com");
        handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_id,
            "del me".into(),
        )
        .await;

        let key = mediamtx::keys::chat_stream(&stream_id);
        let entries = cache.xrevrange(&key, 10).await.unwrap();
        let msg_id = entries[0]
            .fields
            .iter()
            .find(|(f, _)| f == "id")
            .map(|(_, v)| v.clone())
            .unwrap();

        let msgindex_key = mediamtx::keys::chat_msgindex(&stream_id);
        let entry_id = cache.hget(&msgindex_key, &msg_id).await.unwrap().unwrap();
        cache.xdel(&key, &entry_id).await.unwrap();
        cache.hdel(&msgindex_key, &msg_id).await.unwrap();

        let after = cache.xrevrange(&key, 10).await.unwrap();
        assert!(after.is_empty());
        assert!(cache.hget(&msgindex_key, &msg_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn ban_and_unban_flow() {
        let cache = InMemoryCache::new();
        let owner_id = Uuid::new_v4();
        let target = Uuid::new_v4();

        let ban_key = mediamtx::keys::chat_ban(&owner_id, &target);
        let bans_set_key = mediamtx::keys::chat_bans_set(&owner_id);

        cache.set(&ban_key, "1", Some(600)).await.unwrap();
        cache
            .sadd(&bans_set_key, &target.to_string())
            .await
            .unwrap();

        assert!(cache.get(&ban_key).await.unwrap().is_some());
        let members = cache.smembers(&bans_set_key).await.unwrap();
        assert!(members.contains(&target.to_string()));

        cache.del(&ban_key).await.unwrap();
        cache
            .srem(&bans_set_key, &target.to_string())
            .await
            .unwrap();

        assert!(cache.get(&ban_key).await.unwrap().is_none());
        let members = cache.smembers(&bans_set_key).await.unwrap();
        assert!(!members.contains(&target.to_string()));
    }

    #[tokio::test]
    async fn permanent_ban_has_no_ttl() {
        let cache = InMemoryCache::new();
        let owner_id = Uuid::new_v4();
        let target = Uuid::new_v4();

        let ban_key = mediamtx::keys::chat_ban(&owner_id, &target);
        cache.set(&ban_key, "1", None).await.unwrap();

        let ttl = cache.ttl(&ban_key).await.unwrap();
        assert_eq!(ttl, -1);
    }

    /// Ban on stream A (owned by broadcaster X) persists when the same
    /// broadcaster opens a new stream B — the per-broadcaster ban key is
    /// independent of stream_id.
    #[tokio::test]
    async fn ban_persists_across_streams_of_same_broadcaster() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let broadcaster = Uuid::new_v4();

        // Stream A
        let stream_a = Uuid::new_v4();
        seed_active_stream(&cache, &stream_a).await;
        seed_stream_owner(&cache, &stream_a, &broadcaster).await;

        let banned = user(Uuid::new_v4(), "troll@example.com");
        // Ban via broadcaster key (simulates what ban_user_handler does)
        let ban_key = mediamtx::keys::chat_ban(&broadcaster, &banned.id);
        cache.set(&ban_key, "1", None).await.unwrap();

        // Banned on stream A
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&banned),
            stream_a,
            "hi".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::Banned)
        ));

        // Stream B (same broadcaster, new stream_id)
        let stream_b = Uuid::new_v4();
        seed_active_stream(&cache, &stream_b).await;
        seed_stream_owner(&cache, &stream_b, &broadcaster).await;

        // Clear rate-limit so the second call reaches the ban check
        cache
            .del(&mediamtx::keys::chat_ratelimit(&banned.id))
            .await
            .unwrap();

        // Still banned on stream B
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&banned),
            stream_b,
            "hi".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::Banned)
        ));
    }

    /// Bans from different broadcasters are independent.
    #[tokio::test]
    async fn ban_does_not_cross_broadcasters() {
        let cache = InMemoryCache::new();
        let pubsub = InMemoryPubSub::new();
        let broadcaster_x = Uuid::new_v4();
        let broadcaster_y = Uuid::new_v4();

        let stream_x = Uuid::new_v4();
        seed_active_stream(&cache, &stream_x).await;
        seed_stream_owner(&cache, &stream_x, &broadcaster_x).await;

        let stream_y = Uuid::new_v4();
        seed_active_stream(&cache, &stream_y).await;
        seed_stream_owner(&cache, &stream_y, &broadcaster_y).await;

        let u = user(Uuid::new_v4(), "viewer@example.com");
        // Banned only by broadcaster X
        let ban_key = mediamtx::keys::chat_ban(&broadcaster_x, &u.id);
        cache.set(&ban_key, "1", None).await.unwrap();

        // Banned in broadcaster X's stream
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_x,
            "hi".into(),
        )
        .await;
        assert!(matches!(
            out,
            SendChatOutcome::Rejected(ChatErrorReason::Banned)
        ));

        // Clear rate-limit so the second call reaches the ban check
        cache
            .del(&mediamtx::keys::chat_ratelimit(&u.id))
            .await
            .unwrap();

        // Not banned in broadcaster Y's stream
        let out = handle_send_chat(
            &cache,
            &pubsub,
            &mock_uow(),
            Some(&u),
            stream_y,
            "hi".into(),
        )
        .await;
        assert!(matches!(out, SendChatOutcome::Accepted));
    }
}
