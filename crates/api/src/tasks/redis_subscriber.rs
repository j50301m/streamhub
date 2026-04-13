use cache::PubSub;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::ws::manager::WsManager;
use crate::ws::types::{RedisEvent, ServerMessage};

/// Spawn a background task that subscribes to Redis `streamhub:events`
/// and distributes events to local WS clients via WsManager.
pub async fn spawn(
    pubsub: Arc<dyn PubSub>,
    ws_manager: Arc<WsManager>,
    shutdown: CancellationToken,
) {
    let mut rx = match pubsub.subscribe("streamhub:events").await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to subscribe to streamhub:events");
            return;
        }
    };

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("Redis subscriber task shutting down");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(payload) => {
                            match serde_json::from_str::<RedisEvent>(&payload) {
                                Ok(RedisEvent::LiveStreams { data }) => {
                                    ws_manager.update_cached_live_streams(data.clone()).await;
                                    ws_manager.broadcast_to_all(&ServerMessage::LiveStreams { data }).await;
                                }
                                Ok(RedisEvent::ViewerCount { stream_id, count }) => {
                                    ws_manager.update_viewer_count(stream_id, count).await;
                                    let msg = ServerMessage::ViewerCount { stream_id, count };
                                    ws_manager.broadcast_to_stream_subscribers(&stream_id, &msg).await;
                                }
                                Ok(RedisEvent::Reconnect { reason, stream_ids }) => {
                                    let msg = ServerMessage::Reconnect { reason, stream_ids };
                                    ws_manager.broadcast_to_all(&msg).await;
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to parse Redis event");
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "Redis subscriber lagged, missed messages");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::warn!("Redis subscriber channel closed");
                            break;
                        }
                    }
                }
            }
        }
    });

    tracing::info!("Spawned Redis subscriber task for streamhub:events");
}
