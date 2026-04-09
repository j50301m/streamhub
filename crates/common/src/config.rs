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
}
