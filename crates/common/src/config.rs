use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    #[serde(default = "default_database_url")]
    pub database_url: String,

    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_mediamtx_url")]
    pub mediamtx_url: String,
}

fn default_database_url() -> String {
    "postgres://streamhub:streamhub@localhost:5432/streamhub".to_string()
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_mediamtx_url() -> String {
    "http://localhost:9997".to_string()
}

impl AppConfig {
    /// Load config from environment variables.
    /// Env vars are prefixed with `STREAMHUB_` (e.g. `STREAMHUB_DATABASE_URL`).
    pub fn load() -> Result<Self, config::ConfigError> {
        let config = config::Config::builder()
            .add_source(
                config::Environment::with_prefix("STREAMHUB")
                    .separator("_")
                    .try_parsing(true),
            )
            .build()?;
        config.try_deserialize()
    }
}
