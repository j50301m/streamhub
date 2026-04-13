pub mod config;
pub mod error;
pub mod state;

pub use config::AppConfig;
pub use error::AppError;
pub use state::{CacheStore, InMemoryCache, InMemoryPubSub, PubSub};

use metrics_exporter_prometheus::PrometheusHandle;
use repo::UnitOfWork;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use storage::ObjectStorage;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Shared application state passed to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub uow: UnitOfWork,
    pub config: AppConfig,
    pub storage: Option<Arc<dyn ObjectStorage>>,
    pub metrics: PrometheusHandle,
    pub redis_pool: deadpool_redis::Pool,
    pub pubsub: Arc<dyn PubSub>,
    pub cache: Arc<dyn CacheStore>,
    /// Active live thumbnail capture tasks. Key = stream_id, Value = CancellationToken.
    /// Periodically captures HLS frames as thumbnails during live streams.
    /// Tokens are cancelled on unpublish or server shutdown.
    pub live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
}

/// Initialize database connection pool with statement_timeout.
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
