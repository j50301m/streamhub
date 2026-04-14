use anyhow::Result;
use cache::CacheStore;
use entity::stream;
use mediamtx::keys;
use repo::UnitOfWork;
use sea_orm::Set;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Spawn a background task that periodically health-checks all MediaMTX instances.
/// On 3 consecutive failures, marks all streams on that instance as Error and cleans up Redis.
pub fn spawn(
    cache: Arc<dyn CacheStore>,
    instances: Vec<mediamtx::MtxInstance>,
    uow: UnitOfWork,
    live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut checker = mediamtx::HealthChecker::new(cache.clone(), instances);
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("Health check task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let acquired = cache.set_nx(keys::HEALTH_CHECK_LOCK, "1", Some(15)).await.unwrap_or(false);
                    if !acquired {
                        continue;
                    }
                    let newly_failed = checker.check_all().await;

                    for failed_inst in newly_failed {
                        tracing::error!(name = %failed_inst.name, "Handling MTX failure: cleaning up streams");

                        if let Err(e) = handle_mtx_failure(
                            &cache,
                            &failed_inst.name,
                            &uow,
                            &live_tasks,
                        ).await {
                            tracing::error!(name = %failed_inst.name, error = %e, "Failed to handle MTX failure");
                        }
                    }
                }
            }
        }
    });

    tracing::info!("Spawned MediaMTX health check task (10s interval)");
}

/// Handle a failed MTX instance: scan for its streams via session schema, mark
/// them as Error, clean up session keys, cancel thumbnail tasks, reset count.
async fn handle_mtx_failure(
    cache: &Arc<dyn CacheStore>,
    mtx_name: &str,
    uow: &UnitOfWork,
    live_tasks: &Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
) -> Result<()> {
    let live_streams = uow.stream_repo().list_live().await?;
    let live_ids: Vec<Uuid> = live_streams.iter().map(|s| s.id).collect();

    let hits = mediamtx::get_streams_on_mtx(cache.as_ref(), &live_ids, mtx_name).await?;

    for (stream_id, session_id) in hits {
        tracing::warn!(%stream_id, %session_id, mtx_name, "Marking stream as Error due to MTX failure");

        let active = stream::ActiveModel {
            id: Set(stream_id),
            status: Set(stream::StreamStatus::Error),
            ..Default::default()
        };
        if let Err(e) = uow.stream_repo().update(active).await {
            tracing::error!(%stream_id, error = %e, "Failed to update stream status to Error");
        }

        // end_session would DECR mtx count, but we reset count to 0 below anyway.
        // cleanup_stale_session avoids relying on active_session matching.
        if let Err(e) = mediamtx::cleanup_stale_session(cache.as_ref(), &session_id).await {
            tracing::error!(%stream_id, error = %e, "Failed to clean up session");
        }
        // Also drop active_session pointer if it still points at this dead session.
        let active_key = keys::stream_active_session(&stream_id);
        if cache.get(&active_key).await?.as_deref() == Some(&session_id.to_string()) {
            let _ = cache.del(&active_key).await;
        }

        let mut tasks = live_tasks.lock().await;
        if let Some(token) = tasks.remove(&stream_id) {
            token.cancel();
            tracing::info!(%stream_id, "Cancelled thumbnail task for failed MTX stream");
        }
    }

    // Final safety net: reset the failed instance's counter.
    cache
        .set(&keys::mtx_stream_count(mtx_name), "0", None)
        .await?;

    Ok(())
}
