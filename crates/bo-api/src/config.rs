//! Environment-driven configuration for the back-office API.

use cfgloader_rs::*;

/// Back-office API configuration loaded from environment variables at startup.
#[derive(FromEnv, Debug, Clone)]
pub struct BoConfig {
    /// Postgres connection URL (`DATABASE_URL`).
    #[env(
        "DATABASE_URL",
        default = "postgres://streamhub:streamhub@localhost:5433/streamhub"
    )]
    pub database_url: String,

    /// Redis connection URL (`REDIS_URL`).
    #[env("REDIS_URL", default = "redis://localhost:6379")]
    pub redis_url: String,

    /// Secret used to verify JWTs (`JWT_SECRET`).
    #[env("JWT_SECRET", default = "dev-secret-change-in-production")]
    pub jwt_secret: String,

    /// HTTP bind address (`BO_API_HOST`).
    #[env("BO_API_HOST", default = "0.0.0.0")]
    pub host: String,

    /// HTTP bind port (`BO_API_PORT`).
    #[env("BO_API_PORT", default = "8800")]
    pub port: u16,

    /// Comma-separated CORS allowed origins (`BO_API_CORS_ORIGINS`).
    #[env("BO_API_CORS_ORIGINS", default = "http://localhost:3000")]
    pub cors_origins: String,

    // ── Rate Limiting ─────────────────────────────────────────────
    /// General rate limit: max requests per window.
    #[env("RATE_LIMIT_BO_GENERAL_LIMIT", default = "60")]
    pub rate_limit_general_limit: u64,
    /// General rate limit: window in seconds.
    #[env("RATE_LIMIT_BO_GENERAL_WINDOW", default = "60")]
    pub rate_limit_general_window: u64,
}

impl BoConfig {
    /// Returns the configured CORS origins split into a `Vec`.
    pub fn cors_origin_list(&self) -> Vec<String> {
        self.cors_origins
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}
