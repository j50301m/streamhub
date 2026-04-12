use anyhow::Result;
use axum::Router;
use cfgloader_rs::FromEnv;
use common::{AppConfig, AppState};
use metrics_exporter_prometheus::PrometheusHandle;
use opentelemetry_otlp::WithExportConfig;
use repo::UnitOfWork;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
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
        let addr = SocketAddr::new(config.host.parse()?, config.port);

        let live_tasks = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let state = AppState {
            uow: UnitOfWork::new(db),
            config,
            storage,
            metrics: prometheus_handle,
            live_tasks: live_tasks.clone(),
        };

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
        })
    }

    pub async fn run(self) -> Result<()> {
        tracing::info!("Starting server on {}", self.addr);
        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        axum::serve(listener, self.router)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

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
