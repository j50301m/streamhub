use cache::{CacheStore, PubSub};
use mediamtx::MtxInstance;
use repo::UnitOfWork;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ws::manager::WsManager;
use crate::ws::types::{RedisEvent, ServerMessage};

/// Spawn background tasks that subscribe to Redis pub/sub channels and
/// distribute events to local WS clients and handle admin force-end cleanup.
#[allow(clippy::too_many_arguments)]
pub async fn spawn(
    pubsub: Arc<dyn PubSub>,
    ws_manager: Arc<WsManager>,
    cache: Arc<dyn CacheStore>,
    uow: UnitOfWork,
    live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    mtx_instances: Vec<MtxInstance>,
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

    // Subscribe to admin_force_end channel
    let mut force_end_rx = match pubsub
        .subscribe(mediamtx::keys::ADMIN_FORCE_END_CHANNEL)
        .await
    {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to subscribe to admin_force_end channel");
            return;
        }
    };

    // User-suspended listener
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

    // Admin force-end listener: async cleanup lifecycle
    let shutdown3 = shutdown.clone();
    let cache2 = cache.clone();
    let pubsub2 = pubsub.clone();
    let uow2 = uow.clone();
    let live_tasks2 = live_tasks;
    let mtx_instances2 = mtx_instances;
    let ws_mgr3 = ws_manager.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown3.cancelled() => {
                    tracing::info!("Admin force-end subscriber task shutting down");
                    break;
                }
                result = force_end_rx.recv() => {
                    match result {
                        Ok(payload) => {
                            #[derive(serde::Deserialize)]
                            struct AdminForceEndEvent {
                                stream_id: Uuid,
                                #[allow(dead_code)]
                                requested_by: Uuid,
                            }
                            match serde_json::from_str::<AdminForceEndEvent>(&payload) {
                                Ok(event) => {
                                    tracing::info!(stream_id = %event.stream_id, requested_by = %event.requested_by, "Received admin_force_end event");
                                    handle_admin_force_end(
                                        &cache2,
                                        &pubsub2,
                                        &uow2,
                                        &live_tasks2,
                                        &mtx_instances2,
                                        &ws_mgr3,
                                        event.stream_id,
                                    ).await;
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to parse admin_force_end event");
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "Admin force-end subscriber lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::warn!("Admin force-end subscriber channel closed");
                            break;
                        }
                    }
                }
            }
        }
    });

    // Main events listener
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

    tracing::info!(
        "Spawned Redis subscriber tasks for streamhub:events, user_suspended, admin_force_end"
    );
}

/// Async cleanup after an admin force-ends a live stream.
/// Mirrors the unpublish lifecycle: end_session, DECR MTX count, cancel thumbnail,
/// publish live_streams event, unsubscribe chat pub/sub.
async fn handle_admin_force_end(
    cache: &Arc<dyn CacheStore>,
    pubsub: &Arc<dyn PubSub>,
    uow: &UnitOfWork,
    live_tasks: &Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    mtx_instances: &[MtxInstance],
    ws_manager: &Arc<WsManager>,
    stream_id: Uuid,
) {
    // 1. Get active session and end it (clears Redis session keys + DECR MTX count)
    let active_session = mediamtx::get_active_session(cache.as_ref(), &stream_id)
        .await
        .ok()
        .flatten();

    if let Some(session_id) = active_session {
        if let Err(e) = mediamtx::end_session(cache.as_ref(), &session_id).await {
            tracing::error!(error = %e, %stream_id, "Failed to end session during admin force-end");
        }
    }

    // 2. Cancel thumbnail task
    {
        let mut tasks = live_tasks.lock().await;
        if let Some(token) = tasks.remove(&stream_id) {
            token.cancel();
            tracing::info!(%stream_id, "Cancelled thumbnail task for admin force-end");
        }
    }

    // 3. Unsubscribe chat pub/sub for this stream
    if let Err(e) = pubsub
        .unsubscribe(&mediamtx::keys::chat_pubsub_channel(&stream_id))
        .await
    {
        tracing::warn!(error = %e, "Failed to unsubscribe chat pubsub during admin force-end");
    }

    // 4. Publish live_streams event to update all WS clients
    if let Err(e) = publish_live_streams_event(uow, cache, pubsub, mtx_instances, ws_manager).await
    {
        tracing::error!(error = %e, "Failed to publish live_streams event after admin force-end");
    }

    tracing::info!(%stream_id, "Admin force-end cleanup completed");
}

/// Publish the current live-streams list on Redis `streamhub:events`.
/// Simplified version of the one in publish.rs, without needing full AppState.
async fn publish_live_streams_event(
    uow: &UnitOfWork,
    cache: &Arc<dyn CacheStore>,
    pubsub: &Arc<dyn PubSub>,
    mtx_instances: &[MtxInstance],
    _ws_manager: &Arc<WsManager>,
) -> Result<(), anyhow::Error> {
    let live_models = uow.stream_repo().list_live().await?;
    let mut data = Vec::with_capacity(live_models.len());

    for m in live_models {
        let urls =
            mediamtx::resolve_stream_urls(cache.as_ref(), mtx_instances, &m.id, &m.stream_key)
                .await
                .unwrap_or(None);

        let (whep, hls) = match urls {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };

        data.push(crate::ws::types::LiveStreamData {
            id: m.id,
            title: m.title,
            stream_key: m.stream_key,
            status: serde_json::to_value(&m.status)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "unknown".to_string()),
            thumbnail_url: m.thumbnail_url,
            started_at: m.started_at,
            viewer_count: 0,
            urls: crate::ws::types::LiveStreamUrls { whep, hls },
        });
    }

    let event = crate::ws::types::RedisEvent::LiveStreams { data };
    let json = serde_json::to_string(&event)?;
    pubsub.publish("streamhub:events", &json).await?;
    Ok(())
}
