//! streamhub back-office API server.
//!
//! Independent admin binary serving the admin dashboard API and static admin
//! console on port 8800.

use anyhow::Result;
use cache::{CacheStore, PubSub, RedisCacheStore, RedisPubSub};
use cfgloader_rs::FromEnv;
use repo::UnitOfWork;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};

use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod config;
mod extractors;
mod handlers;
mod routes;
mod state;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config = config::BoConfig::load_iter([
        std::path::Path::new(".env.local"),
        std::path::Path::new(".env"),
    ])
    .expect("failed to load bo-api config");

    tracing::info!(port = config.port, "Starting bo-api");

    let db = state::init_db(&config.database_url).await?;

    tracing::info!("Syncing database schema from entities...");
    db.get_schema_registry("entity::*").sync(&db).await?;

    let redis_cfg = deadpool_redis::Config::from_url(&config.redis_url);
    let redis_pool = redis_cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1))?;
    let cache: Arc<dyn CacheStore> = Arc::new(RedisCacheStore::new(redis_pool.clone()));
    let pubsub: Arc<dyn PubSub> = Arc::new(RedisPubSub::new(redis_pool, config.redis_url.clone()));

    let addr = SocketAddr::new(config.host.parse()?, config.port);

    let cors = build_cors(&config);

    let app_state = state::BoAppState {
        uow: UnitOfWork::new(db),
        cache,
        pubsub,
        config,
    };

    let router = axum::Router::new()
        .merge(routes::app_router())
        .layer(cors)
        .layer(TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::new().level(Level::INFO)))
        .with_state(app_state);

    tracing::info!("bo-api listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn build_cors(config: &config::BoConfig) -> CorsLayer {
    let origins = config.cors_origin_list();
    if origins.is_empty() {
        CorsLayer::permissive()
    } else {
        let parsed: Vec<_> = origins.iter().filter_map(|o| o.parse().ok()).collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(parsed))
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    }
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("Shutdown signal received");
}
