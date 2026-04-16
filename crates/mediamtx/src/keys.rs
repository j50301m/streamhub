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

/// Key `chat:{stream_id}:stream` — Redis Stream holding chat messages for a stream.
pub fn chat_stream(stream_id: &Uuid) -> String {
    format!("chat:{stream_id}:stream")
}

/// Key `chat:ratelimit:{user_id}` — per-user chat rate-limit lock (EX 1).
pub fn chat_ratelimit(user_id: &Uuid) -> String {
    format!("chat:ratelimit:{user_id}")
}

/// Pub/sub channel `streamhub:chat:{stream_id}` for cross-instance chat fan-out.
pub fn chat_pubsub_channel(stream_id: &Uuid) -> String {
    format!("streamhub:chat:{stream_id}")
}

/// Key `chat:{stream_id}:msgindex` — HASH mapping UUID v7 msg_id → Redis Stream entry_id.
pub fn chat_msgindex(stream_id: &Uuid) -> String {
    format!("chat:{stream_id}:msgindex")
}

/// Key `chat:ban:{stream_id}:{user_id}` — exists while the user is banned from the stream.
pub fn chat_ban(stream_id: &Uuid, user_id: &Uuid) -> String {
    format!("chat:ban:{stream_id}:{user_id}")
}

/// Key `chat:bans:{stream_id}` — SET of banned user_ids for the stream.
pub fn chat_bans_set(stream_id: &Uuid) -> String {
    format!("chat:bans:{stream_id}")
}

/// Key `stream:{stream_id}:viewer_count` — cached viewer count written by the
/// periodic viewer-count task, readable from any instance.
pub fn viewer_count(stream_id: &Uuid) -> String {
    format!("stream:{stream_id}:viewer_count")
}

/// Key `user:state:{user_id}` — access-state cache. Value is `"active"` (EX 300)
/// or `"suspended"` (no-expire for permanent, EX remaining for temporary).
pub fn user_state(user_id: &Uuid) -> String {
    format!("user:state:{user_id}")
}

/// Pub/sub channel for cross-instance user suspension notifications.
pub const USER_SUSPENDED_CHANNEL: &str = "streamhub:user_suspended";

/// Cluster-wide lock key for the viewer-count refresh task.
pub const VIEWER_COUNT_LOCK: &str = "viewer_count_lock";

/// Cluster-wide lock key for the MTX health-check task.
pub const HEALTH_CHECK_LOCK: &str = "health_check_lock";
