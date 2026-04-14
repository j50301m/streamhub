//! Centralized Redis key formatting for MediaMTX session + routing state.
//!
//! Every key read/write in the codebase must go through one of these helpers —
//! inline `format!("stream:...")` / `format!("session:...")` / `format!("mtx:...")`
//! is forbidden.

use uuid::Uuid;

/// `stream:{stream_id}:active_session` — current active session for a stream.
pub fn stream_active_session(stream_id: &Uuid) -> String {
    format!("stream:{stream_id}:active_session")
}

/// `session:{session_id}:mtx` — which MTX instance a session lives on.
pub fn session_mtx(session_id: &Uuid) -> String {
    format!("session:{session_id}:mtx")
}

/// `session:{session_id}:stream_id` — reverse lookup from session to stream.
pub fn session_stream_id(session_id: &Uuid) -> String {
    format!("session:{session_id}:stream_id")
}

/// `session:{session_id}:started_at` — ISO8601 start timestamp (diagnostic).
pub fn session_started_at(session_id: &Uuid) -> String {
    format!("session:{session_id}:started_at")
}

/// `mtx:{name}:stream_count` — per-instance active stream counter.
pub fn mtx_stream_count(mtx_name: &str) -> String {
    format!("mtx:{mtx_name}:stream_count")
}

/// `mtx:{name}:status` — `"healthy" | "unhealthy" | "draining"`.
pub fn mtx_status(mtx_name: &str) -> String {
    format!("mtx:{mtx_name}:status")
}

pub const VIEWER_COUNT_LOCK: &str = "viewer_count_lock";
pub const HEALTH_CHECK_LOCK: &str = "health_check_lock";
