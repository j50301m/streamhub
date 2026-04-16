//! Shared application state and database initialisation.

use cache::{CacheStore, PubSub};
use mediamtx::MtxInstance;
use metrics_exporter_prometheus::PrometheusHandle;
use repo::UnitOfWork;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use storage::ObjectStorage;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::AppConfig;

/// Shared application state cloned into every route handler.
#[derive(Clone)]
pub struct AppState {
    /// Database access layer (repositories + transaction management).
    pub uow: UnitOfWork,
    /// Parsed environment configuration.
    pub config: AppConfig,
    /// Object storage backend (GCS in production, mock in tests).
    pub storage: Arc<dyn ObjectStorage>,
    /// Prometheus metrics handle exposed on `/metrics`.
    pub metrics: PrometheusHandle,
    /// Raw Redis pool for callers that need commands beyond [`CacheStore`].
    pub redis_pool: deadpool_redis::Pool,
    /// Cache store (session state, MTX counters, stream tokens).
    pub cache: Arc<dyn CacheStore>,
    /// Pub/sub for cross-instance event fan-out.
    pub pubsub: Arc<dyn PubSub>,
    /// Active live-thumbnail capture tasks keyed by `stream_id`.
    ///
    /// Each token is cancelled on unpublish or server shutdown to stop the
    /// associated capture loop.
    pub live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    /// Registered MediaMTX instances available for routing new streams.
    pub mtx_instances: Vec<MtxInstance>,
}

/// Opens the Postgres connection pool used by the API.
///
/// Applies production-oriented defaults: `max_connections=20`, `min=2`,
/// 10s connect timeout, 5min idle timeout, and a 30s server-side
/// `statement_timeout` to bound slow queries.
///
/// # Errors
/// Returns a [`sea_orm::DbErr`] if the pool cannot be established.
pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut opt = ConnectOptions::new(database_url);
    opt.max_connections(20)
        .min_connections(2)
        .connect_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .statement_timeout(Duration::from_secs(30))
        .sqlx_logging(true);
    Database::connect(opt).await
}
