pub mod config;
pub mod error;

pub use config::AppConfig;
pub use error::AppError;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::time::Duration;

/// Shared application state passed to all route handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub mediamtx_url: String,
    pub jwt_secret: String,
    /// Local filesystem path where recordings are stored (for path mapping).
    pub recordings_path: String,
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
