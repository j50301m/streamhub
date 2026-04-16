//! Axum middleware: authentication extractor, rate limiting, and Prometheus metrics tracker.

/// Authentication extractor and user_id injection for rate limiting.
pub mod auth;
/// HTTP request counter and latency histogram middleware.
pub mod metrics;
/// Global unauthed rate-limit middleware (skips `/internal/*`).
pub mod rate_limit;

pub use auth::CurrentUser;
