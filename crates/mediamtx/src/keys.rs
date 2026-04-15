//! Centralized Redis key formatting for MediaMTX session + routing state.
//!
//! Every key read/write in the codebase must go through one of these helpers —
//! inline `format!("stream:...")` / `format!("session:...")` / `format!("mtx:...")`
//! is forbidden.

use uuid::Uuid;

/// Key `stream:{stream_id}:active_session` — the currently active session UUID
/// for a stream, or missing if the stream has no active session.
pub fn stream_active_session(stream_id: &Uuid) -> String {
    format!("stream:{stream_id}:active_session")
}

/// Key `session:{session_id}:mtx` — which MTX instance hosts a session.
pub fn session_mtx(session_id: &Uuid) -> String {
    format!("session:{session_id}:mtx")
}

/// Key `session:{session_id}:stream_id` — reverse lookup from session to stream.
pub fn session_stream_id(session_id: &Uuid) -> String {
    format!("session:{session_id}:stream_id")
}

/// Key `session:{session_id}:started_at` — ISO-8601 session start timestamp
/// (diagnostic only).
pub fn session_started_at(session_id: &Uuid) -> String {
    format!("session:{session_id}:started_at")
}

/// Key `stream_token:{token_hash}` — broadcaster WHIP auth token hash → stream_id.
pub fn stream_token(token_hash: &str) -> String {
    format!("stream_token:{token_hash}")
}

/// Key `mtx:{name}:stream_count` — per-instance active stream counter used by
/// [`super::select_instance`] for load balancing.
pub fn mtx_stream_count(mtx_name: &str) -> String {
    format!("mtx:{mtx_name}:stream_count")
}

/// Key `mtx:{name}:status` — one of `"healthy" | "unhealthy" | "draining"`.
pub fn mtx_status(mtx_name: &str) -> String {
    format!("mtx:{mtx_name}:status")
}

/// Cluster-wide lock key for the viewer-count refresh task.
pub const VIEWER_COUNT_LOCK: &str = "viewer_count_lock";

/// Cluster-wide lock key for the MTX health-check task.
pub const HEALTH_CHECK_LOCK: &str = "health_check_lock";
