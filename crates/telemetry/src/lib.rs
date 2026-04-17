//! Shared observability bootstrap for streamhub HTTP services.
//!
//! Provides a single entry point that wires up:
//! - OpenTelemetry OTLP/gRPC trace exporter
//! - Prometheus recorder + `/metrics` handle
//! - JSON log formatter that injects span fields + active trace_id
//!
//! Both `api` and `bo-api` depend on this crate so they share the exact same
//! observability baseline (log schema, metric names, trace resource format).

mod http_metrics;
mod log_format;

pub use http_metrics::base_http_metrics;
pub use log_format::{JsonWithTraceId, SpanFields, SpanFieldsLayer};
pub use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

// Re-export the `metrics` crate so downstream apps can emit app-specific
// counters / histograms (e.g. `telemetry::metrics_api::counter!(...)`)
// without having to pull it in as a direct dependency.
pub use ::metrics as metrics_api;

use opentelemetry_otlp::WithExportConfig;
use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Errors that can occur during [`init_telemetry`].
///
/// Installation failures (Prometheus recorder / tracing subscriber) are
/// global, one-shot operations — on failure the caller is expected to
/// terminate startup.
#[derive(Debug, Error)]
pub enum TelemetryInitError {
    /// Failed to build the OTLP exporter (invalid endpoint, TLS setup, …).
    #[error("failed to build OTLP trace exporter: {0}")]
    OtlpExporter(#[from] opentelemetry_otlp::ExporterBuildError),

    /// Failed to build or install the Prometheus recorder (e.g. another
    /// recorder is already installed in this process).
    #[error("failed to install Prometheus recorder: {0}")]
    Prometheus(String),

    /// Failed to install the global tracing subscriber (already installed).
    #[error("failed to install tracing subscriber: {0}")]
    TracingSubscriber(String),
}

/// Initialises the global tracing subscriber + Prometheus recorder for a
/// process.
///
/// Wires up:
/// - OTLP trace exporter pushing to `otel_endpoint` (gRPC, batched).
/// - Prometheus recorder with histogram buckets suitable for HTTP latency
///   (5ms … 10s); the returned handle is used by the `/metrics` route.
/// - JSON log layer that merges span fields and the active trace_id into
///   every event so Loki can cross-link to Tempo.
///
/// `service_name` sets the OpenTelemetry resource attribute so the service
/// appears as its own entry in Tempo / Grafana ServiceGraph.
///
/// # Errors
/// Returns a [`TelemetryInitError`] if the OTLP exporter build, Prometheus
/// recorder install, or tracing subscriber install fails. Must only be
/// called once per process — the Prometheus recorder and global tracing
/// subscriber are process-global.
pub fn init_telemetry(
    otel_endpoint: &str,
    service_name: &'static str,
) -> Result<PrometheusHandle, TelemetryInitError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otel_endpoint)
        .build()?;

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name(service_name)
                .build(),
        )
        .build();

    // Set as global provider, then get tracer from global
    opentelemetry::global::set_tracer_provider(provider);
    let tracer = opentelemetry::global::tracer(service_name);

    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .set_buckets(&[
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ])
        .map_err(|e| TelemetryInitError::Prometheus(e.to_string()))?
        .install_recorder()
        .map_err(|e| TelemetryInitError::Prometheus(e.to_string()))?;

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(SpanFieldsLayer)
        .with(tracing_subscriber::fmt::layer().event_format(JsonWithTraceId))
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .try_init()
        .map_err(|e| TelemetryInitError::TracingSubscriber(e.to_string()))?;

    Ok(prometheus_handle)
}
