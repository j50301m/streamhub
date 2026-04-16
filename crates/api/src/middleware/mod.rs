//! Axum middleware: authentication extractor and Prometheus metrics tracker.

mod auth;
/// HTTP request counter and latency histogram middleware.
pub mod metrics;

pub use auth::{AdminUser, CurrentUser};
