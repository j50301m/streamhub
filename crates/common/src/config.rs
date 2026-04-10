use cfgloader_rs::*;

#[derive(FromEnv, Debug, Clone)]
pub struct AppConfig {
    #[env(
        "DATABASE_URL",
        default = "postgres://streamhub:streamhub@localhost:5433/streamhub"
    )]
    pub database_url: String,

    #[env("HOST", default = "0.0.0.0")]
    pub host: String,

    #[env("PORT", default = "8080")]
    pub port: u16,

    #[env("MEDIAMTX_URL", default = "http://localhost:9997")]
    pub mediamtx_url: String,

    #[env("JWT_SECRET", default = "dev-secret-change-in-production")]
    pub jwt_secret: String,

    /// Local path where MediaMTX recordings are stored.
    /// In Docker, MediaMTX writes to /recordings; on host, this maps to ./recordings.
    #[env("RECORDINGS_PATH", default = "./recordings")]
    pub recordings_path: String,

    #[env("STORAGE_ENABLED", default = "false")]
    pub storage_enabled: String,

    #[env("GCS_BUCKET", default = "streamhub-recordings-dev")]
    pub gcs_bucket: String,

    #[env("GCS_ENDPOINT", default = "")]
    pub gcs_endpoint: String,

    #[env("GCS_CREDENTIALS_PATH", default = "")]
    pub gcs_credentials_path: String,

    #[env("TRANSCODER_ENABLED", default = "false")]
    pub transcoder_enabled: String,

    #[env("TRANSCODER_PROJECT_ID", default = "")]
    pub transcoder_project_id: String,

    #[env("TRANSCODER_LOCATION", default = "asia-east1")]
    pub transcoder_location: String,

    #[env("PUBSUB_VERIFY_TOKEN", default = "")]
    pub pubsub_verify_token: String,
}

impl AppConfig {
    pub fn storage_enabled(&self) -> bool {
        self.storage_enabled.eq_ignore_ascii_case("true") || self.storage_enabled == "1"
    }

    pub fn transcoder_enabled(&self) -> bool {
        self.transcoder_enabled.eq_ignore_ascii_case("true") || self.transcoder_enabled == "1"
    }

    pub fn gcs_endpoint_opt(&self) -> Option<&str> {
        if self.gcs_endpoint.is_empty() {
            None
        } else {
            Some(&self.gcs_endpoint)
        }
    }

    pub fn gcs_credentials_path_opt(&self) -> Option<&str> {
        if self.gcs_credentials_path.is_empty() {
            None
        } else {
            Some(&self.gcs_credentials_path)
        }
    }
}
