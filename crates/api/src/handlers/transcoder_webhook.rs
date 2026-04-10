use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use common::AppState;
use entity::stream;
use sea_orm::Set;
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PubSubPush {
    pub message: PubSubMessage,
}

#[derive(Debug, Deserialize)]
pub struct PubSubMessage {
    pub data: String, // base64 encoded
    #[allow(dead_code)]
    pub attributes: Option<serde_json::Value>,
}

/// Transcoder job state change event (decoded from Pub/Sub message data).
#[derive(Debug, Deserialize)]
struct TranscoderEvent {
    job: TranscoderJob,
}

#[derive(Debug, Deserialize)]
struct TranscoderJob {
    #[allow(dead_code)]
    name: String,
    state: String,
    #[serde(default)]
    labels: HashMap<String, String>,
}

/// POST /internal/hooks/transcoder-complete
///
/// Receives Pub/Sub push notifications for GCP Transcoder API job state changes.
/// Updates the stream's vod_status based on the job result.
pub(crate) async fn transcoder_webhook(
    State(state): State<AppState>,
    Json(payload): Json<PubSubPush>,
) -> Result<StatusCode, StatusCode> {
    // Optional: verify Pub/Sub token
    // (In production, use Pub/Sub push authentication via OIDC instead)
    let verify_token = &state.config.pubsub_verify_token;
    if !verify_token.is_empty() {
        // Token verification would normally come from query params or headers.
        // For now we just log that it's configured.
        tracing::debug!("Pub/Sub verify token is configured");
    }

    // Decode base64 message data
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&payload.message.data)
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to decode Pub/Sub message data");
            StatusCode::BAD_REQUEST
        })?;

    let event: TranscoderEvent = serde_json::from_slice(&decoded).map_err(|e| {
        tracing::error!(error = %e, "Failed to parse Transcoder event JSON");
        StatusCode::BAD_REQUEST
    })?;

    let stream_id_str = event.job.labels.get("stream_id").ok_or_else(|| {
        tracing::error!(job_name = %event.job.name, "Transcoder event missing stream_id label");
        StatusCode::BAD_REQUEST
    })?;

    let stream_id: Uuid = stream_id_str.parse().map_err(|e| {
        tracing::error!(error = %e, stream_id = %stream_id_str, "Invalid stream_id UUID");
        StatusCode::BAD_REQUEST
    })?;

    tracing::info!(
        stream_id = %stream_id,
        job_name = %event.job.name,
        state = %event.job.state,
        "Received Transcoder job state change"
    );

    match event.job.state.as_str() {
        "SUCCEEDED" => {
            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Ready),
                ..Default::default()
            };
            state.uow.stream_repo().update(active).await.map_err(|e| {
                tracing::error!(error = %e, "Failed to update stream vod_status to Ready");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            tracing::info!(stream_id = %stream_id, "VOD transcoding succeeded, vod_status=Ready");
        }
        "FAILED" => {
            let active = stream::ActiveModel {
                id: Set(stream_id),
                vod_status: Set(stream::VodStatus::Failed),
                ..Default::default()
            };
            state.uow.stream_repo().update(active).await.map_err(|e| {
                tracing::error!(error = %e, "Failed to update stream vod_status to Failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            tracing::warn!(stream_id = %stream_id, "VOD transcoding failed, vod_status=Failed");
        }
        other => {
            // Intermediate states (PENDING, RUNNING, etc.) — acknowledge but don't update DB
            tracing::debug!(stream_id = %stream_id, state = %other, "Ignoring intermediate Transcoder job state");
        }
    }

    // Always return 200 so Pub/Sub doesn't retry
    Ok(StatusCode::OK)
}
