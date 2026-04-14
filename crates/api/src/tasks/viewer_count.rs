use anyhow::Result;
use cache::{CacheStore, PubSub};
use mediamtx::keys;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ws::manager::WsManager;
use crate::ws::types::RedisEvent;

/// Spawn a periodic task that polls MediaMTX API for viewer (reader) counts
/// and publishes changes via Redis PubSub.
pub fn spawn(
    cache: Arc<dyn CacheStore>,
    pubsub: Arc<dyn PubSub>,
    instances: Vec<mediamtx::MtxInstance>,
    ws_manager: Arc<WsManager>,
    interval_secs: u64,
    shutdown: CancellationToken,
) {
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let lock_ttl = interval_secs * 3 / 2;

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("Viewer count task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let acquired = cache.set_nx(keys::VIEWER_COUNT_LOCK, "1", Some(lock_ttl)).await.unwrap_or(false);
                    if !acquired {
                        continue;
                    }
                    if let Err(e) = poll_viewer_counts(
                        &http_client,
                        &cache,
                        &pubsub,
                        &instances,
                        &ws_manager,
                    ).await {
                        tracing::warn!(error = %e, "Viewer count poll failed");
                    }
                }
            }
        }
    });

    tracing::info!(interval_secs, "Spawned viewer count periodic task");
}

/// One round of viewer count polling across all healthy MTX instances.
async fn poll_viewer_counts(
    client: &reqwest::Client,
    cache: &Arc<dyn CacheStore>,
    pubsub: &Arc<dyn PubSub>,
    instances: &[mediamtx::MtxInstance],
    ws_manager: &Arc<WsManager>,
) -> Result<()> {
    for inst in instances {
        // Skip instances that are not healthy (draining / unhealthy / missing)
        let status = cache.get(&keys::mtx_status(&inst.name)).await?;
        if status.as_deref() != Some("healthy") {
            continue;
        }

        let url = format!("{}/v3/paths/list", inst.internal_api);
        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(_) | Err(_) => continue,
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        // MediaMTX v3 response: { "items": [{ "name": "stream-key", "readers": [{"type": "..."}, ...], ... }] }
        if let Some(items) = body.get("items").and_then(|v| v.as_array()) {
            for item in items {
                let path_name = match item.get("name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => continue,
                };

                let reader_count = item
                    .get("readers")
                    .and_then(|r| {
                        // MediaMTX v3: readers is an array of reader objects
                        if let Some(arr) = r.as_array() {
                            // Only count actual viewers, exclude internal muxers
                            let count = arr
                                .iter()
                                .filter(|reader| {
                                    reader
                                        .get("type")
                                        .and_then(|t| t.as_str())
                                        .map(|t| t != "hlsMuxer" && t != "rtmpConn")
                                        .unwrap_or(true)
                                })
                                .count();
                            Some(count as u64)
                        } else if let Some(obj) = r.as_object() {
                            // Fallback: {"readers": {"count": N}}
                            obj.get("count").and_then(|c| c.as_u64())
                        } else {
                            r.as_u64()
                        }
                    })
                    .unwrap_or(0) as u32;

                // stream_key == stream_id (UUID v4), so parse directly.
                let stream_id = match path_name.parse::<Uuid>() {
                    Ok(id) => id,
                    Err(_) => continue,
                };

                if ws_manager
                    .update_viewer_count(stream_id, reader_count)
                    .await
                {
                    let event = RedisEvent::ViewerCount {
                        stream_id,
                        count: reader_count,
                    };
                    if let Ok(json) = serde_json::to_string(&event) {
                        let _ = pubsub.publish("streamhub:events", &json).await;
                    }
                }
            }
        }
    }

    Ok(())
}
