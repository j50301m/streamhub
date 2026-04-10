use anyhow::Result;
use axum::Router;
use axum::routing::get;
use cfgloader_rs::FromEnv;
use common::AppState;
use metrics_exporter_prometheus::PrometheusHandle;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use repo::UnitOfWork;
use std::net::SocketAddr;
use std::sync::Arc;
use storage::GcsStorage;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub mod extractors;
pub mod handlers;
pub mod middleware;
mod routes;

#[cfg(test)]
mod tests;

fn init_telemetry(otel_endpoint: &str) -> Result<PrometheusHandle> {
    // OpenTelemetry OTLP exporter → Tempo
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

    let tracer = provider.tracer("streamhub-api");

    // Prometheus metrics recorder
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder");

    // Tracing subscriber: OpenTelemetry layer + JSON fmt + env filter
    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(tracing_subscriber::fmt::layer().json())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    Ok(prometheus_handle)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load config from .env file (falls back to environment variables + defaults)
    let config = common::AppConfig::load_iter([
        std::path::Path::new(".env.local"),
        std::path::Path::new(".env"),
    ])
    .unwrap_or_else(|e| {
        eprintln!("Failed to load .env file ({e}), using env vars / defaults");
        common::AppConfig::load(std::path::Path::new("/dev/null"))
            .expect("config with defaults should always load")
    });

    let prometheus_handle = init_telemetry(&config.otel_endpoint)?;

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
        .route(
            "/metrics",
            get({
                let handle = prometheus_handle;
                move || {
                    let h = handle.clone();
                    async move { h.render() }
                }
            }),
        )
        .layer(axum::middleware::from_fn(
            middleware::metrics::track_metrics,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    tracing::info!("Starting server on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
