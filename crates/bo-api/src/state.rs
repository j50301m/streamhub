//! Shared application state for the back-office API.

use cache::{CacheStore, PubSub};
use repo::UnitOfWork;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::sync::Arc;
use std::time::Duration;

use crate::config::BoConfig;

/// Shared state cloned into every bo-api route handler.
#[derive(Clone)]
pub struct BoAppState {
    /// Database access layer (repositories + transaction management).
    pub uow: UnitOfWork,
    /// Cache store (viewer counts, session state).
    pub cache: Arc<dyn CacheStore>,
    /// Pub/sub for cross-instance event fan-out (user_suspended).
    pub pubsub: Arc<dyn PubSub>,
    /// Parsed environment configuration.
    pub config: BoConfig,
}

/// Opens the Postgres connection pool used by bo-api.
pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut opt = ConnectOptions::new(database_url);
    opt.max_connections(10)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .statement_timeout(Duration::from_secs(30))
        .sqlx_logging(true);
    Database::connect(opt).await
}
