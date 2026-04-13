use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server → Client messages sent over WebSocket.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Full list of currently live streams. Sent on connect + on publish/unpublish.
    LiveStreams { data: Vec<LiveStreamData> },
    /// Viewer count update for a specific stream.
    ViewerCount { stream_id: Uuid, count: u32 },
    /// Reconnect request — client should reconnect affected streams.
    Reconnect {
        reason: String,
        stream_ids: Vec<Uuid>,
    },
}

/// Compact live stream info for WS push.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStreamData {
    pub id: Uuid,
    pub title: Option<String>,
    pub stream_key: String,
    pub status: String,
    pub thumbnail_url: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub viewer_count: u32,
    pub urls: LiveStreamUrls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStreamUrls {
    pub whep: Option<String>,
    pub hls: Option<String>,
}

/// Client → Server messages received over WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Subscribe to viewer count updates for a specific stream.
    Subscribe { stream_id: Uuid },
    /// Unsubscribe from viewer count updates for a specific stream.
    Unsubscribe { stream_id: Uuid },
}

/// Redis event envelope — published on `streamhub:events` channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RedisEvent {
    LiveStreams {
        data: Vec<LiveStreamData>,
    },
    ViewerCount {
        stream_id: Uuid,
        count: u32,
    },
    Reconnect {
        reason: String,
        stream_ids: Vec<Uuid>,
    },
}

impl From<RedisEvent> for ServerMessage {
    fn from(event: RedisEvent) -> Self {
        match event {
            RedisEvent::LiveStreams { data } => ServerMessage::LiveStreams { data },
            RedisEvent::ViewerCount { stream_id, count } => {
                ServerMessage::ViewerCount { stream_id, count }
            }
            RedisEvent::Reconnect { reason, stream_ids } => {
                ServerMessage::Reconnect { reason, stream_ids }
            }
        }
    }
}
