//! Environment-driven application configuration.

use cfgloader_rs::*;

/// Application configuration loaded from environment variables at startup.
///
/// Each field has a development-friendly default; production deployments are
/// expected to override the relevant variables (DATABASE_URL, JWT_SECRET,
/// GCS_*, TRANSCODER_*, REDIS_URL, MEDIAMTX_INSTANCES, ...).
#[derive(FromEnv, Debug, Clone)]
pub struct AppConfig {
    /// Postgres connection URL (`DATABASE_URL`).
    #[env(
        "DATABASE_URL",
        default = "postgres://streamhub:streamhub@localhost:5433/streamhub"
    )]
    pub database_url: String,

    /// HTTP bind address (`HOST`).
    #[env("HOST", default = "0.0.0.0")]
    pub host: String,

    /// HTTP bind port (`PORT`).
    #[env("PORT", default = "8080")]
    pub port: u16,

    /// Secret used to sign JWTs (`JWT_SECRET`). Must be overridden in production.
    #[env("JWT_SECRET", default = "dev-secret-change-in-production")]
    pub jwt_secret: String,

    /// Filesystem path where MediaMTX writes recordings. In Docker this is
    /// `/recordings`; on the host it maps to `./recordings`.
    #[env("RECORDINGS_PATH", default = "./recordings")]
    pub recordings_path: String,

    /// Filesystem path where live thumbnail snapshots are written.
    #[env("THUMBNAILS_PATH", default = "/thumbnails")]
    pub thumbnails_path: String,

    /// GCS bucket for recordings and VOD assets.
    #[env("GCS_BUCKET", default = "streamhub-recordings-dev")]
    pub gcs_bucket: String,

    /// Optional GCS endpoint override (e.g. fake-gcs-server). Empty = real GCS.
    #[env("GCS_ENDPOINT", default = "")]
    pub gcs_endpoint: String,

    /// Path to a GCP service-account JSON file. Empty = use ADC.
    #[env("GCS_CREDENTIALS_PATH", default = "")]
    pub gcs_credentials_path: String,

    /// Enables Transcoder API integration when set to `"true"` or `"1"`.
    #[env("TRANSCODER_ENABLED", default = "false")]
    pub transcoder_enabled: String,

    /// GCP project ID for the Transcoder API.
    #[env("TRANSCODER_PROJECT_ID", default = "")]
    pub transcoder_project_id: String,

    /// GCP region for Transcoder jobs.
    #[env("TRANSCODER_LOCATION", default = "asia-east1")]
    pub transcoder_location: String,

    /// Shared secret Pub/Sub push subscriptions must present in the
    /// `?token=` query string. Empty disables verification (dev only).
    #[env("PUBSUB_VERIFY_TOKEN", default = "")]
    pub pubsub_verify_token: String,

    /// OTLP gRPC endpoint for traces and metrics.
    #[env("OTEL_EXPORTER_OTLP_ENDPOINT", default = "http://localhost:4317")]
    pub otel_endpoint: String,

    /// Interval in seconds between live thumbnail captures.
    #[env("THUMBNAIL_CAPTURE_INTERVAL_SECS", default = "60")]
    pub thumbnail_capture_interval_secs: u64,

    /// Redis connection URL used by cache, pubsub, and the deadpool.
    #[env("REDIS_URL", default = "redis://localhost:6379")]
    pub redis_url: String,

    /// Interval in seconds between viewer-count refreshes from MediaMTX.
    #[env("VIEWER_COUNT_INTERVAL_SECS", default = "10")]
    pub viewer_count_interval_secs: u64,

    /// JSON array describing available MediaMTX instances; see
    /// [`mediamtx::MtxInstance`] for the shape.
    #[env("MEDIAMTX_INSTANCES", default = "")]
    pub mediamtx_instances_json: String,
}

impl AppConfig {
    /// Returns whether the Transcoder pipeline should be invoked on recording
    /// completion.
    pub fn transcoder_enabled(&self) -> bool {
        self.transcoder_enabled.eq_ignore_ascii_case("true") || self.transcoder_enabled == "1"
    }

    /// Returns the GCS endpoint override as `Some(&str)`, or `None` when the
    /// real GCS endpoint should be used.
    pub fn gcs_endpoint_opt(&self) -> Option<&str> {
        if self.gcs_endpoint.is_empty() {
            None
        } else {
            Some(&self.gcs_endpoint)
        }
    }

    /// Returns the GCS credentials path as `Some(&str)`, or `None` when ADC
    /// should be used instead.
    pub fn gcs_credentials_path_opt(&self) -> Option<&str> {
        if self.gcs_credentials_path.is_empty() {
            None
        } else {
            Some(&self.gcs_credentials_path)
        }
    }
}
