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

    // Subscribe to user_suspended channel for cross-instance disconnect
    let mut user_suspended_rx = match pubsub
        .subscribe(mediamtx::keys::USER_SUSPENDED_CHANNEL)
        .await
    {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to subscribe to user_suspended channel");
            return;
        }
    };

    let ws_mgr2 = ws_manager.clone();
    let shutdown2 = shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown2.cancelled() => {
                    tracing::info!("User-suspended subscriber task shutting down");
                    break;
                }
                result = user_suspended_rx.recv() => {
                    match result {
                        Ok(payload) => {
                            #[derive(serde::Deserialize)]
                            struct UserSuspendedEvent {
                                user_id: uuid::Uuid,
                            }
                            match serde_json::from_str::<UserSuspendedEvent>(&payload) {
                                Ok(event) => {
                                    tracing::info!(user_id = %event.user_id, "Received user_suspended event, disconnecting WS");
                                    ws_mgr2.disconnect_user(&event.user_id).await;
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to parse user_suspended event");
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "User-suspended subscriber lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::warn!("User-suspended subscriber channel closed");
                            break;
                        }
                    }
                }
            }
        }
    });

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

    tracing::info!("Spawned Redis subscriber tasks for streamhub:events and user_suspended");
}
