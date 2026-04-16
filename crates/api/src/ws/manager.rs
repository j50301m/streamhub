use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::ws::Message;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use crate::ws::types::{LiveStreamData, ServerMessage};

/// A single WebSocket connection.
pub struct WsConnection {
    /// Outbound queue feeding this connection's send half.
    pub tx: mpsc::UnboundedSender<Message>,
}

/// Manages all local WebSocket connections on this API instance.
/// Handles per-stream subscriptions and cached state for fast initial pushes.
pub struct WsManager {
    /// All active connections: conn_id → WsConnection
    connections: RwLock<HashMap<Uuid, WsConnection>>,
    /// Per-stream subscribers: stream_id → set of conn_ids
    stream_subscribers: RwLock<HashMap<Uuid, HashSet<Uuid>>>,
    /// Per-channel chat subscribers: stream_id → set of conn_ids. Fed from
    /// the Redis `streamhub:chat:{stream_id}` pub/sub channel.
    chat_subscribers: RwLock<HashMap<Uuid, HashSet<Uuid>>>,
    /// Stream IDs whose chat pub/sub forwarder task is already running on this
    /// instance. Used by `ensure_chat_pubsub_task` for idempotency — without
    /// this guard, every WS that subscribes would spawn its own forwarder and
    /// each chat message would be fanned out N times.
    chat_pubsub_active: RwLock<HashSet<Uuid>>,
    /// Cached live streams list (pushed to new connections immediately)
    cached_live_streams: RwLock<Vec<LiveStreamData>>,
    /// Cached viewer counts per stream
    cached_viewer_counts: RwLock<HashMap<Uuid, u32>>,
    /// User → connections mapping (for suspend disconnect).
    /// Anonymous WS (no JWT) are not tracked here.
    user_connections: RwLock<HashMap<Uuid, HashSet<Uuid>>>,
}

impl WsManager {
    /// Create a new manager wrapped in `Arc` for shared access across tasks.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            connections: RwLock::new(HashMap::new()),
            stream_subscribers: RwLock::new(HashMap::new()),
            chat_subscribers: RwLock::new(HashMap::new()),
            chat_pubsub_active: RwLock::new(HashSet::new()),
            cached_live_streams: RwLock::new(Vec::new()),
            cached_viewer_counts: RwLock::new(HashMap::new()),
            user_connections: RwLock::new(HashMap::new()),
        })
    }

    /// Register a new WebSocket connection. Returns the conn_id.
    pub async fn add_connection(&self, conn_id: Uuid, tx: mpsc::UnboundedSender<Message>) {
        let conn = WsConnection { tx };
        self.connections.write().await.insert(conn_id, conn);
    }

    /// Track an authenticated user's connection for suspend-disconnect.
    pub async fn track_user_connection(&self, user_id: Uuid, conn_id: Uuid) {
        self.user_connections
            .write()
            .await
            .entry(user_id)
            .or_default()
            .insert(conn_id);
    }

    /// Remove a WebSocket connection and clean up all its subscriptions.
    pub async fn remove_connection(&self, conn_id: &Uuid) {
        self.connections.write().await.remove(conn_id);
        let mut subs = self.stream_subscribers.write().await;
        for subscribers in subs.values_mut() {
            subscribers.remove(conn_id);
        }
        // Remove empty sets
        subs.retain(|_, v| !v.is_empty());
        drop(subs);

        let mut chat = self.chat_subscribers.write().await;
        for subscribers in chat.values_mut() {
            subscribers.remove(conn_id);
        }
        chat.retain(|_, v| !v.is_empty());
        drop(chat);

        // Clean up user_connections
        let mut uc = self.user_connections.write().await;
        for conn_ids in uc.values_mut() {
            conn_ids.remove(conn_id);
        }
        uc.retain(|_, v| !v.is_empty());
    }

    /// Disconnect all WS connections belonging to a user (used for suspend).
    /// Sends a `SessionTerminated` message before closing.
    pub async fn disconnect_user(&self, user_id: &Uuid) {
        let conn_ids: Vec<Uuid> = {
            let uc = self.user_connections.read().await;
            uc.get(user_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect()
        };

        if conn_ids.is_empty() {
            return;
        }

        let msg = ServerMessage::SessionTerminated {
            reason: "ACCOUNT_SUSPENDED".to_string(),
        };
        let text = match serde_json::to_string(&msg) {
            Ok(t) => t,
            Err(_) => return,
        };

        let conns = self.connections.read().await;
        for conn_id in &conn_ids {
            if let Some(conn) = conns.get(conn_id) {
                // Send session_terminated then close
                let _ = conn.tx.send(Message::Text(text.clone().into()));
                let _ = conn.tx.send(Message::Close(None));
            }
        }
        drop(conns);

        // Remove from user_connections
        self.user_connections.write().await.remove(user_id);

        tracing::info!(user_id = %user_id, count = conn_ids.len(), "Disconnected user WS connections");
    }

    /// Subscribe a connection to a chat room.
    pub async fn subscribe_chat(&self, conn_id: Uuid, stream_id: Uuid) {
        self.chat_subscribers
            .write()
            .await
            .entry(stream_id)
            .or_default()
            .insert(conn_id);
    }

    /// Unsubscribe a connection from a chat room.
    pub async fn unsubscribe_chat(&self, conn_id: &Uuid, stream_id: &Uuid) {
        let mut chat = self.chat_subscribers.write().await;
        if let Some(subscribers) = chat.get_mut(stream_id) {
            subscribers.remove(conn_id);
            if subscribers.is_empty() {
                chat.remove(stream_id);
            }
        }
    }

    /// Atomically claim the chat pub/sub forwarder slot for a stream.
    /// Returns `true` if this caller is the first — meaning it should spawn
    /// the forwarder task. Subsequent callers get `false` and must skip.
    pub async fn try_claim_chat_pubsub(&self, stream_id: Uuid) -> bool {
        self.chat_pubsub_active.write().await.insert(stream_id)
    }

    /// Release the chat pub/sub forwarder slot. Called when the forwarder task
    /// exits (e.g. after `pubsub.unsubscribe` causes its receiver to close),
    /// so a future stream session can spawn a fresh one.
    pub async fn release_chat_pubsub(&self, stream_id: &Uuid) {
        self.chat_pubsub_active.write().await.remove(stream_id);
    }

    /// Broadcast a serialized JSON payload to all chat subscribers of a room.
    pub async fn broadcast_chat_to_room(&self, stream_id: &Uuid, payload: &str) {
        let chat = self.chat_subscribers.read().await;
        let Some(subscriber_ids) = chat.get(stream_id) else {
            return;
        };
        let conns = self.connections.read().await;
        for conn_id in subscriber_ids {
            if let Some(conn) = conns.get(conn_id) {
                let _ = conn.tx.send(Message::Text(payload.to_string().into()));
            }
        }
    }

    /// Send a `ServerMessage` to a single connection by id.
    pub async fn send_to(&self, conn_id: &Uuid, msg: &ServerMessage) {
        self.send_to_connection(conn_id, msg).await;
    }

    /// Subscribe a connection to a specific stream's events.
    pub async fn subscribe_stream(&self, conn_id: Uuid, stream_id: Uuid) {
        self.stream_subscribers
            .write()
            .await
            .entry(stream_id)
            .or_default()
            .insert(conn_id);

        // Immediately send current viewer count if cached
        let counts = self.cached_viewer_counts.read().await;
        if let Some(&count) = counts.get(&stream_id) {
            let msg = ServerMessage::ViewerCount { stream_id, count };
            self.send_to_connection(&conn_id, &msg).await;
        }
    }

    /// Unsubscribe a connection from a specific stream's events.
    pub async fn unsubscribe_stream(&self, conn_id: &Uuid, stream_id: &Uuid) {
        let mut subs = self.stream_subscribers.write().await;
        if let Some(subscribers) = subs.get_mut(stream_id) {
            subscribers.remove(conn_id);
            if subscribers.is_empty() {
                subs.remove(stream_id);
            }
        }
    }

    /// Get cached live streams for initial push to new connections.
    pub async fn get_cached_live_streams(&self) -> Vec<LiveStreamData> {
        self.cached_live_streams.read().await.clone()
    }

    /// Update cached live streams and return the new list.
    pub async fn update_cached_live_streams(&self, streams: Vec<LiveStreamData>) {
        *self.cached_live_streams.write().await = streams;
    }

    /// Update cached viewer count for a stream. Returns true if the value changed.
    pub async fn update_viewer_count(&self, stream_id: Uuid, count: u32) -> bool {
        let mut counts = self.cached_viewer_counts.write().await;
        let prev = counts.insert(stream_id, count);
        prev != Some(count)
    }

    /// Remove a stream from the viewer count cache (e.g. when it goes offline).
    pub async fn remove_viewer_count(&self, stream_id: &Uuid) {
        self.cached_viewer_counts.write().await.remove(stream_id);
    }

    /// Broadcast a message to ALL connected clients.
    pub async fn broadcast_to_all(&self, msg: &ServerMessage) {
        let text = match serde_json::to_string(msg) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, "Failed to serialize WS message");
                return;
            }
        };
        let conns = self.connections.read().await;
        for conn in conns.values() {
            let _ = conn.tx.send(Message::Text(text.clone().into()));
        }
    }

    /// Broadcast a message to connections subscribed to a specific stream.
    pub async fn broadcast_to_stream_subscribers(&self, stream_id: &Uuid, msg: &ServerMessage) {
        let text = match serde_json::to_string(msg) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, "Failed to serialize WS message");
                return;
            }
        };
        let subs = self.stream_subscribers.read().await;
        if let Some(subscriber_ids) = subs.get(stream_id) {
            let conns = self.connections.read().await;
            for conn_id in subscriber_ids {
                if let Some(conn) = conns.get(conn_id) {
                    let _ = conn.tx.send(Message::Text(text.clone().into()));
                }
            }
        }
    }

    /// Send a message to a single connection.
    pub async fn send_to_connection(&self, conn_id: &Uuid, msg: &ServerMessage) {
        let text = match serde_json::to_string(msg) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, "Failed to serialize WS message");
                return;
            }
        };
        let conns = self.connections.read().await;
        if let Some(conn) = conns.get(conn_id) {
            let _ = conn.tx.send(Message::Text(text.into()));
        }
    }

    /// Get the number of active connections.
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }
}
