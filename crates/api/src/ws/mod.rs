//! WebSocket support: connection manager and wire-format types.

/// Per-process WebSocket connection manager.
pub mod manager;
/// WebSocket message types (server ↔ client) and Redis pubsub envelope.
pub mod types;
