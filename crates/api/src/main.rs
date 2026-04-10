use anyhow::Result;
use axum::Router;
use cfgloader_rs::FromEnv;
use common::AppState;
use repo::UnitOfWork;
use std::net::SocketAddr;
use std::sync::Arc;
use storage::GcsStorage;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

pub mod extractors;
pub mod handlers;
pub mod middleware;
mod routes;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Load config from .env file (falls back to environment variables + defaults)
    let config = common::AppConfig::load_iter([
        std::path::Path::new(".env.local"),
        std::path::Path::new(".env"),
    ])
    .unwrap_or_else(|e| {
        tracing::warn!("Failed to load .env file ({e}), using env vars / defaults");
        // If no .env file, load from environment variables only
        common::AppConfig::load(std::path::Path::new("/dev/null"))
            .expect("config with defaults should always load")
    });

    tracing::info!("Connecting to database...");
    let db = common::init_db(&config.database_url).await?;

    tracing::info!("Syncing database schema from entities...");
    db.get_schema_registry("entity::*").sync(&db).await?;

    let addr = SocketAddr::new(config.host.parse()?, config.port);

    let storage: Option<Arc<dyn storage::ObjectStorage>> = if config.storage_enabled() {
        let gcs = GcsStorage::new(
            &config.gcs_bucket,
            config.gcs_endpoint_opt(),
            config.gcs_credentials_path_opt(),
        )
        .await?;
        gcs.ensure_bucket().await?;
        tracing::info!(bucket = %config.gcs_bucket, "GCS storage enabled");
        Some(Arc::new(gcs))
    } else {
        tracing::info!("GCS storage disabled, using local file serving");
        None
    };

    let state = AppState {
        uow: UnitOfWork::new(db),
        config,
        storage,
    };

    let app = Router::new()
        .merge(routes::app_router())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    tracing::info!("Starting server on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
