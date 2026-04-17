//! Admin console handlers.

/// User authentication: login / refresh.
pub mod auth;
/// Platform dashboard summary endpoint.
pub mod dashboard;
/// Prometheus `/metrics` scrape endpoint.
pub mod metrics;
/// Cross-stream moderation: bans, chat viewer.
pub mod moderation;
/// Stream management: list, detail, force-end.
pub mod streams;
/// User management: list, role change, suspend, unsuspend.
pub mod users;
