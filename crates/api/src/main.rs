use anyhow::Result;
use axum::Router;
use std::net::SocketAddr;
use streamhub_common::AppState;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod routes;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Load config
    let config = streamhub_common::AppConfig::load().unwrap_or_else(|e| {
        tracing::warn!("Failed to load config, using defaults: {e}");
        // Return default config
        serde_json::from_str("{}").expect("default config should deserialize")
    });

    tracing::info!("Connecting to database...");
    let db = streamhub_common::init_db(&config.database_url).await?;

    tracing::info!("Running migrations...");
    {
        use streamhub_migration::MigratorTrait;
        streamhub_migration::Migrator::up(&db, None).await?;
    }

    let state = AppState {
        db,
        mediamtx_url: config.mediamtx_url.clone(),
    };

    let app = Router::new()
        .merge(routes::health_routes())
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
