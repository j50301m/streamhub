//! Axum middleware: authentication extractor and rate limiting.
//!
//! Shared HTTP metrics live in the `telemetry` crate; app-specific metric
//! emissions (e.g. `rate_limit_hits_total`) live with their business logic.

/// Authentication extractor and user_id injection for rate limiting.
pub mod auth;
/// Global unauthed rate-limit middleware (skips `/internal/*`).
pub mod rate_limit;

pub use auth::CurrentUser;
