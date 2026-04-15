use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server → Client messages sent over WebSocket.
#[derive(Debug, Clone, Serialize)]
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
    },
    /// Viewer-count change for a stream.
    ViewerCount {
        /// Target stream UUID.
        stream_id: Uuid,
        /// Current viewer count.
        count: u32,
    },
    /// Tell affected clients to reconnect (e.g. MTX draining).
    Reconnect {
        /// Free-form reason string.
        reason: String,
        /// Streams affected.
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
