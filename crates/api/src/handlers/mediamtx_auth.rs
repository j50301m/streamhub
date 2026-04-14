use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use common::AppState;
use entity::stream;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct MediaMtxAuthRequest {
    pub ip: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub path: String,
    pub protocol: Option<String>,
    pub id: Option<String>,
    pub action: String,
    pub query: Option<String>,
}

/// POST /internal/auth
/// MediaMTX HTTP auth callback.
/// - action=publish -> validate stream token from query param
/// - action=read -> check stream exists and is Live
#[tracing::instrument(skip(state, payload), fields(path = %payload.path, action = %payload.action))]
pub(crate) async fn mediamtx_auth(
    State(state): State<AppState>,
    Json(payload): Json<MediaMtxAuthRequest>,
) -> StatusCode {
    tracing::info!(
        path = %payload.path,
        action = %payload.action,
        "MediaMTX auth request"
    );

    match payload.action.as_str() {
        "publish" => handle_publish_auth(&state, &payload).await,
        "read" => handle_read_auth(&state, &payload).await,
        // Allow other actions (e.g., "playback", "api") by default
        _ => StatusCode::OK,
    }
}

async fn handle_publish_auth(state: &AppState, payload: &MediaMtxAuthRequest) -> StatusCode {
    // Extract token from query string: "token=xxx"
    let raw_token = match extract_token_from_query(payload.query.as_deref()) {
        Some(t) => t,
        None => {
            tracing::warn!("publish auth: no token in query");
            return StatusCode::UNAUTHORIZED;
        }
    };

    // Find the stream by path (stream_key)
    let stream_model = match state.uow.stream_repo().find_by_key(&payload.path).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            tracing::warn!(path = %payload.path, "publish auth: stream not found");
            return StatusCode::NOT_FOUND;
        }
        Err(e) => {
            tracing::error!("publish auth db error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Hash the provided token and look it up in Redis.
    let token_hash = auth::token::hash_token(&raw_token);
    let key = mediamtx::keys::stream_token(&token_hash);

    let cached_stream_id = match state.cache.get(&key).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            tracing::warn!("publish auth: token not found or expired");
            return StatusCode::UNAUTHORIZED;
        }
        Err(e) => {
            tracing::error!("publish auth cache error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    if cached_stream_id != stream_model.id.to_string() {
        tracing::warn!("publish auth: token does not match stream");
        return StatusCode::UNAUTHORIZED;
    }

    StatusCode::OK
}

async fn handle_read_auth(state: &AppState, payload: &MediaMtxAuthRequest) -> StatusCode {
    // For read (WHEP/HLS), just check stream exists and is Live
    match state.uow.stream_repo().find_by_key(&payload.path).await {
        Ok(Some(s)) if s.status == stream::StreamStatus::Live => StatusCode::OK,
        Ok(Some(_)) => {
            tracing::info!(path = %payload.path, "read auth: stream not live");
            StatusCode::NOT_FOUND
        }
        Ok(None) => {
            tracing::info!(path = %payload.path, "read auth: stream not found");
            StatusCode::NOT_FOUND
        }
        Err(e) => {
            tracing::error!("read auth db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn extract_token_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("token=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
