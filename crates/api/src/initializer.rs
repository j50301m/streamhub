use anyhow::Result;
use axum::Router;
use cache::{CacheStore, RedisCacheStore};
use cfgloader_rs::FromEnv;
use common::{AppConfig, AppState};
use entity::stream;
use metrics_exporter_prometheus::PrometheusHandle;
use opentelemetry_otlp::WithExportConfig;
use repo::UnitOfWork;
use sea_orm::Set;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use storage::GcsStorage;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

use crate::middleware;
use crate::routes;

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

        let state = AppState {
            uow: UnitOfWork::new(db),
            config,
            storage,
            metrics: prometheus_handle,
            redis_pool,
            cache,
            live_tasks: live_tasks.clone(),
            mtx_instances,
        };

        let shutdown_token = CancellationToken::new();

        // Spawn health check background task for MediaMTX instances
        if !state.mtx_instances.is_empty() {
            spawn_health_check_task(
                state.cache.clone(),
                state.mtx_instances.clone(),
                state.uow.clone(),
                state.live_tasks.clone(),
                shutdown_token.clone(),
            );
        }

        let router = Router::new()
            .merge(routes::app_router())
            .layer(axum::middleware::from_fn(
                middleware::metrics::track_metrics,
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
        axum::serve(listener, self.router)
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
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otel_endpoint)
        .build()?;

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name("streamhub-api")
                .build(),
        )
        .build();

    // Set as global provider, then get tracer from global
    opentelemetry::global::set_tracer_provider(provider);
    let tracer = opentelemetry::global::tracer("streamhub-api");

    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .set_buckets(&[
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ])
        .expect("invalid histogram buckets")
        .install_recorder()
        .expect("failed to install Prometheus recorder");

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(crate::log_format::SpanFieldsLayer)
        .with(tracing_subscriber::fmt::layer().event_format(crate::log_format::JsonWithTraceId))
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    Ok(prometheus_handle)
}

async fn init_db(config: &AppConfig) -> Result<sea_orm::DatabaseConnection> {
    tracing::info!("Connecting to database...");
    let db = common::init_db(&config.database_url).await?;

    tracing::info!("Syncing database schema from entities...");
    db.get_schema_registry("entity::*").sync(&db).await?;

    Ok(db)
}

fn init_redis(redis_url: &str) -> Result<deadpool_redis::Pool> {
    tracing::info!("Connecting to Redis...");
    let cfg = deadpool_redis::Config::from_url(redis_url);
    let pool = cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1))?;
    Ok(pool)
}

async fn init_storage(config: &AppConfig) -> Result<Option<Arc<dyn storage::ObjectStorage>>> {
    if config.storage_enabled() {
        let gcs = GcsStorage::new(
            &config.gcs_bucket,
            config.gcs_endpoint_opt(),
            config.gcs_credentials_path_opt(),
        )
        .await?;
        gcs.ensure_bucket().await?;
        tracing::info!(bucket = %config.gcs_bucket, "GCS storage enabled");
        Ok(Some(Arc::new(gcs)))
    } else {
        tracing::info!("GCS storage disabled, using local file serving");
        Ok(None)
    }
}

/// Spawn a background task that periodically health-checks all MediaMTX instances.
/// On 3 consecutive failures, marks all streams on that instance as Error and cleans up Redis.
fn spawn_health_check_task(
    cache: Arc<dyn CacheStore>,
    instances: Vec<mediamtx::MtxInstance>,
    uow: UnitOfWork,
    live_tasks: Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut checker = mediamtx::HealthChecker::new(cache.clone(), instances);
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("Health check task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let newly_failed = checker.check_all().await;

                    for failed_inst in newly_failed {
                        tracing::error!(name = %failed_inst.name, "Handling MTX failure: cleaning up streams");

                        // Scan Redis for streams mapped to this instance
                        // We need to use SCAN pattern: stream:*:mtx
                        // Since CacheStore doesn't support SCAN, we'll use the pool directly
                        if let Err(e) = handle_mtx_failure(
                            &cache,
                            &failed_inst.name,
                            &uow,
                            &live_tasks,
                        ).await {
                            tracing::error!(name = %failed_inst.name, error = %e, "Failed to handle MTX failure");
                        }
                    }
                }
            }
        }
    });

    tracing::info!("Spawned MediaMTX health check task (10s interval)");
}

/// Handle a failed MTX instance: scan for its streams, mark them as Error, cleanup Redis.
async fn handle_mtx_failure(
    cache: &Arc<dyn CacheStore>,
    mtx_name: &str,
    uow: &UnitOfWork,
    live_tasks: &Arc<tokio::sync::Mutex<HashMap<Uuid, CancellationToken>>>,
) -> Result<()> {
    // Find all live streams and check which ones are on this MTX
    let live_streams = uow.stream_repo().list_live().await?;

    for s in live_streams {
        let stream_id_str = s.id.to_string();
        let mapped_mtx = cache.get(&format!("stream:{stream_id_str}:mtx")).await?;

        if mapped_mtx.as_deref() == Some(mtx_name) {
            tracing::warn!(stream_id = %s.id, mtx_name, "Marking stream as Error due to MTX failure");

            // Update DB: stream status = Error
            let active = stream::ActiveModel {
                id: Set(s.id),
                status: Set(stream::StreamStatus::Error),
                ..Default::default()
            };
            if let Err(e) = uow.stream_repo().update(active).await {
                tracing::error!(stream_id = %s.id, error = %e, "Failed to update stream status to Error");
            }

            // Remove Redis mapping
            if let Err(e) = mediamtx::remove_stream_mapping(cache.as_ref(), &stream_id_str).await {
                tracing::error!(stream_id = %s.id, error = %e, "Failed to remove stream mapping");
            }

            // Cancel thumbnail task
            let mut tasks = live_tasks.lock().await;
            if let Some(token) = tasks.remove(&s.id) {
                token.cancel();
                tracing::info!(stream_id = %s.id, "Cancelled thumbnail task for failed MTX stream");
            }
        }
    }

    // Reset stream count to 0
    cache
        .set(&format!("mtx:{mtx_name}:stream_count"), "0", None)
        .await?;

    Ok(())
}
