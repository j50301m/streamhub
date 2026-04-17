//! streamhub back-office API server.
//!
//! Independent admin binary serving the admin dashboard API and static admin
//! console on port 8800.

use anyhow::Result;
use cache::{CacheStore, PubSub, RedisCacheStore, RedisPubSub};
use cfgloader_rs::FromEnv;
use rate_limit::{RateLimitLayer, RateLimitMode, RateLimitPolicy, RedisRateLimiter};
use repo::UnitOfWork;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};

use tower_http::trace::TraceLayer;

mod config;
mod extractors;
mod handlers;
mod routes;
mod state;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::BoConfig::load_iter([
        std::path::Path::new(".env.local"),
        std::path::Path::new(".env"),
    ])
    .expect("failed to load bo-api config");

    // Install OTel + Prometheus + JSON log subscriber before any
    // `tracing::info!` fires. Returns a handle used by the /metrics route.
    let prometheus_handle = telemetry::init_telemetry(&config.otel_endpoint, "streamhub-bo-api")?;

    tracing::info!(port = config.port, "Starting bo-api");

    let db = state::init_db(&config.database_url).await?;

    tracing::info!("Syncing database schema from entities...");
    db.get_schema_registry("entity::*").sync(&db).await?;

    let redis_cfg = deadpool_redis::Config::from_url(&config.redis_url);
    let redis_pool = redis_cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1))?;
    let cache: Arc<dyn CacheStore> = Arc::new(RedisCacheStore::new(redis_pool.clone()));
    // Rate limiter — bo-api uses user_id key (all requests are authed)
    let rate_limiter: Arc<dyn rate_limit::RateLimiter> =
        Arc::new(RedisRateLimiter::new(redis_pool.clone()));
    let pubsub: Arc<dyn PubSub> = Arc::new(RedisPubSub::new(redis_pool, config.redis_url.clone()));

    let addr = SocketAddr::new(config.host.parse()?, config.port);

    let cors = build_cors(&config);

    let bo_general_policy = RateLimitPolicy {
        name: "bo_general".into(),
        limit: config.rate_limit_general_limit,
        window_secs: config.rate_limit_general_window,
        key_prefix: "ratelimit:bo_general".into(),
    };

    let app_state = state::BoAppState {
        uow: UnitOfWork::new(db),
        cache,
        pubsub,
        metrics: prometheus_handle,
        config,
    };

    // Authed router — all admin routes sit behind JWT + rate limit.
    let authed_router = routes::app_router()
        .layer(RateLimitLayer::new(
            rate_limiter,
            bo_general_policy,
            RateLimitMode::UserIdOnly,
        ))
        .layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            inject_user_id_extension,
        ));

    // Unauthed scrape endpoint — Prometheus polls `/metrics` without JWT.
    // Kept outside the authed layer stack so it is never gated by auth
    // or rate limit.
    let metrics_router = routes::metrics_router();

    let router = axum::Router::new()
        .merge(authed_router)
        .merge(metrics_router)
        // Base HTTP metrics — counts every request including /metrics itself.
        .layer(axum::middleware::from_fn(telemetry::base_http_metrics))
        .layer(cors)
        .layer(TraceLayer::new_for_http().make_span_with(telemetry::http_make_span))
        .with_state(app_state);

    tracing::info!("bo-api listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
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

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("Shutdown signal received");
}

/// Lightweight middleware that extracts user_id from a valid JWT and inserts
/// it as a [`rate_limit::RateLimitUserId`] extension for rate limiting.
async fn inject_user_id_extension(
    axum::extract::State(state): axum::extract::State<state::BoAppState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(header) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = header.strip_prefix("Bearer ") {
            if let Ok(claims) = auth::jwt::verify_token(token, &state.config.jwt_secret) {
                if claims.typ == "access" {
                    req.extensions_mut()
                        .insert(rate_limit::RateLimitUserId(claims.sub.to_string()));
                }
            }
        }
    }
    next.run(req).await
}
