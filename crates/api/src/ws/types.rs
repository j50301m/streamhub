use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A chat message delivered to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessagePayload {
    /// Redis Stream entry id (millisecond timestamp + sequence).
    pub id: String,
    /// Stream (chat room) this message belongs to.
    pub stream_id: Uuid,
    /// Sender user UUID.
    pub user_id: Uuid,
    /// Sender display name (email local-part fallback).
    pub display_name: String,
    /// Message body (plain text — client must HTML-escape before rendering).
    pub content: String,
    /// Timestamp (unix millis) parsed from the entry id.
    pub ts_ms: i64,
}

/// Server → Client messages sent over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Full list of currently live streams. Sent on connect and whenever a
    /// stream publishes or unpublishes.
    LiveStreams {
        /// Current live streams.
        data: Vec<LiveStreamData>,
    },
    /// Viewer-count update for a specific stream.
    ViewerCount {
        /// Target stream UUID.
        stream_id: Uuid,
        /// Current viewer count.
        count: u32,
    },
    /// Reconnect request — clients watching the listed streams should tear
    /// down and reconnect so they migrate to a healthy MediaMTX instance.
    Reconnect {
        /// Free-form reason (e.g. `"server_maintenance"`).
        reason: String,
        /// Streams affected by the reconnect.
        stream_ids: Vec<Uuid>,
    },
    /// Recent chat messages pushed immediately after `subscribe_chat`.
    ChatHistory {
        /// Target stream.
        stream_id: Uuid,
        /// Messages ordered oldest → newest.
        messages: Vec<ChatMessagePayload>,
    },
    /// Real-time chat message.
    ChatMessage {
        /// Target stream (chat room).
        stream_id: Uuid,
        /// Delivered message.
        message: ChatMessagePayload,
    },
    /// A chat message was deleted by a moderator / admin.
    ChatMessageDeleted {
        /// Stream whose chat room this deletion applies to.
        stream_id: Uuid,
        /// UUID v7 message id that was deleted.
        msg_id: String,
    },
    /// Chat-level error (rate limit, length, unauthorized, unknown stream).
    ChatError {
        /// Optional stream context if available.
        stream_id: Option<Uuid>,
        /// Machine-readable reason.
        reason: ChatErrorReason,
    },
    /// Session terminated — the user's account has been suspended and all
    /// connections must close. Frontend should clear tokens and show a message.
    SessionTerminated {
        /// Machine-readable reason (e.g. `"ACCOUNT_SUSPENDED"`).
        reason: String,
    },
    /// Stream force-ended by an admin. Broadcaster should stop reconnecting
    /// and treat the current stream as closed.
    StreamForceEnded {
        /// Target stream UUID.
        stream_id: Uuid,
        /// Machine-readable reason.
        reason: String,
    },
}

/// Reasons a `send_chat` may be rejected by the server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChatErrorReason {
    /// Sent more than 1 msg/sec per user.
    RateLimited,
    /// Content exceeds 500 chars or is empty after trimming.
    TooLong,
    /// WebSocket is not authenticated (no JWT provided on connect).
    Unauthorized,
    /// The `stream_id` is not a currently active stream.
    UnknownStream,
    /// The user is banned from this chat room.
    Banned,
    /// Internal error (Redis unavailable, serialization failure, etc.).
    Unknown,
}

/// Compact live-stream record pushed to WS clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStreamData {
    /// Stream UUID.
    pub id: Uuid,
    /// Display title.
    pub title: Option<String>,
    /// MediaMTX path key.
    pub stream_key: String,
    /// Serialized stream status (lowercase).
    pub status: String,
    /// Thumbnail URL.
    pub thumbnail_url: Option<String>,
    /// Time the stream went live.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last known viewer count.
    pub viewer_count: u32,
    /// Playback URLs for this stream.
    pub urls: LiveStreamUrls,
}

/// Playback URLs attached to a `LiveStreamData`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStreamUrls {
    /// WHEP URL (low-latency playback).
    pub whep: Option<String>,
    /// LL-HLS URL (mass playback).
    pub hls: Option<String>,
}

/// Client → Server messages received over WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Subscribe to viewer-count updates for a specific stream.
    Subscribe {
        /// Stream to subscribe to.
        stream_id: Uuid,
    },
    /// Unsubscribe from viewer-count updates for a specific stream.
    Unsubscribe {
        /// Stream to unsubscribe from.
        stream_id: Uuid,
    },
    /// Join a chat room and receive recent history + live messages.
    SubscribeChat {
        /// Chat room (stream) to join.
        stream_id: Uuid,
    },
    /// Leave a chat room.
    UnsubscribeChat {
        /// Chat room (stream) to leave.
        stream_id: Uuid,
    },
    /// Send a chat message. Requires the WebSocket to be authenticated.
    SendChat {
        /// Target chat room.
        stream_id: Uuid,
        /// Message content (max 500 chars, trimmed, non-empty).
        content: String,
    },
}

/// Internal pub/sub wrapper that carries trace propagation metadata without
/// changing the client-facing WebSocket message schema.
///
/// # Design tradeoff
///
/// This envelope flattens `ServerMessage` so old pub/sub payloads (pre
/// SPEC-036, emitted as a bare `ServerMessage` JSON) still deserialize via
/// the subscriber's fallback path. That fallback path (`chat.rs`) also
/// deserializes directly into `ServerMessage`, so **any future change to
/// `ServerMessage`'s serde markers is implicitly a breaking change for the
/// pub/sub fallback as well**. This coupling is intentional and temporary:
/// once all API instances are guaranteed to be on the envelope format, the
/// bare `ServerMessage` fallback in `chat.rs` can be removed and this note
/// deleted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracedServerMessage {
    /// The original WebSocket payload delivered to clients.
    #[serde(flatten)]
    pub message: ServerMessage,
    /// Optional W3C trace context used only for internal pub/sub propagation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}

/// Envelope for events published on Redis `streamhub:events`.
///
/// Every API instance subscribes to this channel and re-broadcasts the event
/// to its local WS clients, so a single publisher reaches all connected viewers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RedisEvent {
    /// Refreshed list of live streams.
    LiveStreams {
        /// Current live streams.
        data: Vec<LiveStreamData>,
        /// W3C traceparent from the publishing span.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        traceparent: Option<String>,
    },
    /// Viewer-count change for a stream.
    ViewerCount {
        /// Target stream UUID.
        stream_id: Uuid,
        /// Current viewer count.
        count: u32,
        /// W3C traceparent from the publishing span.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        traceparent: Option<String>,
    },
    /// Tell affected clients to reconnect (e.g. MTX draining).
    Reconnect {
        /// Free-form reason string.
        reason: String,
        /// Streams affected.
        stream_ids: Vec<Uuid>,
        /// W3C traceparent from the publishing span.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        traceparent: Option<String>,
    },
}

impl From<RedisEvent> for ServerMessage {
    fn from(event: RedisEvent) -> Self {
        match event {
            RedisEvent::LiveStreams {
                data,
                traceparent: _,
            } => ServerMessage::LiveStreams { data },
            RedisEvent::ViewerCount {
                stream_id,
                count,
                traceparent: _,
            } => ServerMessage::ViewerCount { stream_id, count },
            RedisEvent::Reconnect {
                reason,
                stream_ids,
                traceparent: _,
            } => ServerMessage::Reconnect { reason, stream_ids },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_event_without_traceparent_deserializes() {
        let payload = r#"{"type":"viewer_count","stream_id":"550e8400-e29b-41d4-a716-446655440000","count":7}"#;
        let event: RedisEvent = serde_json::from_str(payload).expect("old payload should parse");

        match event {
            RedisEvent::ViewerCount {
                stream_id,
                count,
                traceparent,
            } => {
                assert_eq!(
                    stream_id,
                    Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
                );
                assert_eq!(count, 7);
                assert!(traceparent.is_none());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn traced_server_message_without_traceparent_deserializes() {
        let payload = r#"{"type":"chat_message_deleted","stream_id":"550e8400-e29b-41d4-a716-446655440000","msg_id":"msg-1"}"#;
        let envelope: TracedServerMessage =
            serde_json::from_str(payload).expect("old chat payload should parse");

        assert!(envelope.traceparent.is_none());
        match envelope.message {
            ServerMessage::ChatMessageDeleted { stream_id, msg_id } => {
                assert_eq!(
                    stream_id,
                    Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
                );
                assert_eq!(msg_id, "msg-1");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
