use crate::config::AppConfig;
use crate::state::AppState;
use anyhow::Result;
use axum::Router;
use cache::{CacheStore, PubSub, RedisCacheStore, RedisPubSub};
use cfgloader_rs::FromEnv;
use rate_limit::{RateLimitLayer, RateLimitMode, RateLimitPolicy, RedisRateLimiter};
use repo::UnitOfWork;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use storage::GcsStorage;
use telemetry::PrometheusHandle;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;
use uuid::Uuid;

use crate::middleware;
use crate::routes;
use crate::ws::manager::WsManager;

pub struct App {
    router: Router,
    addr: SocketAddr,
    /// Active live thumbnail capture tasks (server-side HLS periodic capture).
    /// Key = stream_id, Value = CancellationToken to cancel on unpublish or shutdown.
    live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    /// Cancellation token for the health check background task.
    shutdown_token: CancellationToken,
}

impl App {
    pub async fn init() -> Result<Self> {
        let config = AppConfig::load_iter([
            std::path::Path::new(".env.local"),
            std::path::Path::new(".env"),
        ])
        .expect("failed to load config");

        let prometheus_handle = init_telemetry(&config.otel_endpoint)?;
        let db = init_db(&config).await?;
        let storage = init_storage(&config).await?;
        let redis_pool = init_redis(&config.redis_url)?;
        let addr = SocketAddr::new(config.host.parse()?, config.port);

        let live_tasks = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mtx_instances = mediamtx::parse_mtx_instances(&config.mediamtx_instances_json);
        tracing::info!(count = mtx_instances.len(), "Loaded MediaMTX instances");
        let cache: Arc<dyn CacheStore> = Arc::new(RedisCacheStore::new(redis_pool.clone()));
        let pubsub: Arc<dyn PubSub> = Arc::new(RedisPubSub::new(
            redis_pool.clone(),
            config.redis_url.clone(),
        ));

        let viewer_count_interval = config.viewer_count_interval_secs;

        // Create rate limiter before moving redis_pool into state
        let rate_limiter: Arc<dyn rate_limit::RateLimiter> =
            Arc::new(RedisRateLimiter::new(redis_pool.clone()));

        let general_unauthed_policy = RateLimitPolicy {
            name: "api_general_unauthed".into(),
            limit: config.rate_limit_general_unauthed_limit,
            window_secs: config.rate_limit_general_unauthed_window,
            key_prefix: "ratelimit:api_general_unauthed".into(),
        };

        let general_authed_policy = RateLimitPolicy {
            name: "api_general_authed".into(),
            limit: config.rate_limit_general_authed_limit,
            window_secs: config.rate_limit_general_authed_window,
            key_prefix: "ratelimit:api_general_authed".into(),
        };

        let chat_rate_limit_policy = RateLimitPolicy {
            name: "chat".into(),
            limit: config.rate_limit_chat_limit,
            window_secs: config.rate_limit_chat_window,
            key_prefix: "ratelimit:chat".into(),
        };

        let refresh_rate_limit_policy = RateLimitPolicy {
            name: "refresh".into(),
            limit: config.rate_limit_refresh_limit,
            window_secs: config.rate_limit_refresh_window,
            key_prefix: "ratelimit:refresh".into(),
        };

        let app_router = routes::app_router(rate_limiter.clone(), &config);

        let state = AppState {
            uow: UnitOfWork::new(db),
            config,
            storage,
            metrics: prometheus_handle,
            redis_pool,
            cache,
            pubsub,
            live_tasks: live_tasks.clone(),
            mtx_instances,
            rate_limiter: rate_limiter.clone(),
            chat_rate_limit_policy,
            refresh_rate_limit_policy,
        };

        let shutdown_token = CancellationToken::new();
        let ws_manager = WsManager::new();

        // Spawn all background tasks
        crate::tasks::spawn_all(
            state.cache.clone(),
            state.pubsub.clone(),
            state.mtx_instances.clone(),
            state.uow.clone(),
            state.live_tasks.clone(),
            ws_manager.clone(),
            viewer_count_interval,
            shutdown_token.clone(),
        )
        .await;

        let router = Router::new()
            .merge(app_router)
            .layer(axum::Extension(ws_manager))
            .layer(axum::middleware::from_fn(telemetry::base_http_metrics))
            // General authed rate limit: only for requests with a resolved user_id
            .layer(RateLimitLayer::new(
                rate_limiter.clone(),
                general_authed_policy,
                RateLimitMode::UserIdOnly,
            ))
            // General unauthed rate limit: IP key, skips /internal/* and
            // requests that already resolved to an authenticated user.
            .layer(axum::middleware::from_fn_with_state(
                (rate_limiter.clone(), general_unauthed_policy),
                middleware::rate_limit::unauthed_rate_limit,
            ))
            // Inject user_id extension from JWT before either general rate
            // limit runs, so authed requests do not consume the unauthed IP
            // budget and can be keyed by user_id instead.
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                middleware::auth::inject_user_id_extension,
            ))
            .layer(CorsLayer::permissive())
            .layer(
                TraceLayer::new_for_http()
                    .make_span_with(DefaultMakeSpan::new().level(Level::INFO)),
            )
            .with_state(state);

        Ok(Self {
            router,
            addr,
            live_tasks,
            shutdown_token,
        })
    }

    pub async fn run(self) -> Result<()> {
        tracing::info!("Starting server on {}", self.addr);
        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        axum::serve(
            listener,
            self.router
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await?;

        // Cancel health check task
        self.shutdown_token.cancel();

        // Cancel all active live thumbnail tasks on shutdown
        let tasks = self.live_tasks.lock().await;
        for (stream_id, token) in tasks.iter() {
            tracing::info!(%stream_id, "Cancelling live thumbnail task on shutdown");
            token.cancel();
        }
        drop(tasks);

        Ok(())
    }
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("Shutdown signal received");
}

fn init_telemetry(otel_endpoint: &str) -> Result<PrometheusHandle> {
    telemetry::init_telemetry(otel_endpoint, "streamhub-api").map_err(Into::into)
}

async fn init_db(config: &AppConfig) -> Result<sea_orm::DatabaseConnection> {
    tracing::info!("Connecting to database...");
    let db = crate::state::init_db(&config.database_url).await?;

    tracing::info!("Syncing database schema from entities...");
    db.get_schema_registry("entity::*").sync(&db).await?;

    // SPEC-020: stream_tokens moved to Redis; drop the legacy table if present.
    use sea_orm::ConnectionTrait;
    db.execute_unprepared("DROP TABLE IF EXISTS stream_tokens")
        .await?;

    Ok(db)
}

fn init_redis(redis_url: &str) -> Result<deadpool_redis::Pool> {
    tracing::info!("Connecting to Redis...");
    let cfg = deadpool_redis::Config::from_url(redis_url);
    let pool = cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1))?;
    Ok(pool)
}

async fn init_storage(config: &AppConfig) -> Result<Arc<dyn storage::ObjectStorage>> {
    let gcs = GcsStorage::new(
        &config.gcs_bucket,
        config.gcs_endpoint_opt(),
        config.gcs_credentials_path_opt(),
    )
    .await?;
    gcs.ensure_bucket().await?;
    tracing::info!(bucket = %config.gcs_bucket, "GCS storage initialized");
    Ok(Arc::new(gcs))
}
