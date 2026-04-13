mod health_check;
mod redis_subscriber;
mod viewer_count;

use cache::{CacheStore, PubSub};
use repo::UnitOfWork;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ws::manager::WsManager;

/// Spawn all background tasks. Call once during initialization.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_all(
    cache: Arc<dyn CacheStore>,
    pubsub: Arc<dyn PubSub>,
    instances: Vec<mediamtx::MtxInstance>,
    uow: UnitOfWork,
    live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    ws_manager: Arc<WsManager>,
    viewer_count_interval: u64,
    shutdown: CancellationToken,
) {
    if !instances.is_empty() {
        health_check::spawn(
            cache.clone(),
            instances.clone(),
            uow,
            live_tasks,
            shutdown.clone(),
        );

        viewer_count::spawn(
            cache.clone(),
            pubsub.clone(),
            instances,
            ws_manager.clone(),
            viewer_count_interval,
            shutdown.clone(),
        );
    }

    redis_subscriber::spawn(pubsub, ws_manager, shutdown).await;
}
