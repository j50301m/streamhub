use anyhow::Result;
use cache::CacheStore;
use entity::stream;
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
                    let acquired = cache.set_nx("health_check_lock", "1", Some(15)).await.unwrap_or(false);
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

/// Handle a failed MTX instance: scan for its streams, mark them as Error, cleanup Redis.
async fn handle_mtx_failure(
    cache: &Arc<dyn CacheStore>,
    mtx_name: &str,
    uow: &UnitOfWork,
    live_tasks: &Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
) -> Result<()> {
    let live_streams = uow.stream_repo().list_live().await?;

    for s in live_streams {
        let stream_id_str = s.id.to_string();
        let mapped_mtx = cache.get(&format!("stream:{stream_id_str}:mtx")).await?;

        if mapped_mtx.as_deref() == Some(mtx_name) {
            tracing::warn!(stream_id = %s.id, mtx_name, "Marking stream as Error due to MTX failure");

            let active = stream::ActiveModel {
                id: Set(s.id),
                status: Set(stream::StreamStatus::Error),
                ..Default::default()
            };
            if let Err(e) = uow.stream_repo().update(active).await {
                tracing::error!(stream_id = %s.id, error = %e, "Failed to update stream status to Error");
            }

            if let Err(e) = mediamtx::remove_stream_mapping(cache.as_ref(), &stream_id_str).await {
                tracing::error!(stream_id = %s.id, error = %e, "Failed to remove stream mapping");
            }

            let mut tasks = live_tasks.lock().await;
            if let Some(token) = tasks.remove(&s.id) {
                token.cancel();
                tracing::info!(stream_id = %s.id, "Cancelled thumbnail task for failed MTX stream");
            }
        }
    }

    cache
        .set(&format!("mtx:{mtx_name}:stream_count"), "0", None)
        .await?;

    Ok(())
}
