use anyhow::Result;
use axum::Router;
use cfgloader_rs::FromEnv;
use common::AppState;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

pub mod middleware;
mod routes;

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

    let state = AppState {
        db,
        mediamtx_url: config.mediamtx_url.clone(),
        jwt_secret: config.jwt_secret.clone(),
        recordings_path: config.recordings_path.clone(),
    };

    let app = Router::new()
        .merge(routes::health_routes())
        .merge(routes::auth_routes())
        .merge(routes::stream_routes())
        .merge(routes::hook_routes())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::new(config.host.parse()?, config.port);
    tracing::info!("Starting server on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
