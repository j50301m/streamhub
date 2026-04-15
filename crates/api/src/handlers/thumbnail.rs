use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use common::{AppError, AppState};
use entity::stream;
use sea_orm::Set;
use serde::Serialize;
use uuid::Uuid;

use crate::middleware::CurrentUser;

use super::streams::DataResponse;

/// Response for `POST /v1/streams/:id/thumbnail`.
#[derive(Debug, Serialize)]
pub struct ThumbnailResponse {
    /// Public URL of the uploaded thumbnail.
    pub thumbnail_url: String,
}

/// `POST /v1/streams/:id/thumbnail` — upload a JPEG poster for the stream.
///
/// Body is the raw JPEG bytes (no multipart). Stored as
/// `streams/{key}/live-thumb.jpg` in the object store and written back to
/// `streams.thumbnail_url`. Body is capped at 2 MiB at the router layer.
///
/// # Errors
/// - 401 if unauthenticated
/// - 403 if the caller is not the stream owner
/// - 404 `STREAM_NOT_FOUND`
/// - 409 `STREAM_NOT_LIVE_OR_VOD_READY` outside those two states
/// - 500 on filesystem / storage / DB failure
#[tracing::instrument(skip(state, body), fields(stream_id = %id, user_id = %current_user.id))]
pub(crate) async fn upload_thumbnail(
    current_user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Result<Json<DataResponse<ThumbnailResponse>>, AppError> {
    let txn = state.uow.begin().await?;

    let existing = txn
        .stream_repo()
        .find_by_id_for_update(id)
        .await?
        .ok_or_else(|| AppError::NotFound("STREAM_NOT_FOUND".to_string()))?;

    if existing.user_id != Some(current_user.id) {
        return Err(AppError::Forbidden("not the stream owner".to_string()));
    }

    // Only allow upload when Live or VodReady
    let is_live = existing.status == stream::StreamStatus::Live;
    let is_vod_ready = existing.vod_status == stream::VodStatus::Ready;
    if !is_live && !is_vod_ready {
        return Err(AppError::Conflict(
            "STREAM_NOT_LIVE_OR_VOD_READY".to_string(),
        ));
    }

    let stream_key = &existing.stream_key;
    let store = &state.storage;

    let tmp_dir = tempfile::tempdir().map_err(|e| {
        tracing::error!(error = %e, "Failed to create temp dir");
        AppError::Internal("failed to create temp dir".to_string())
    })?;
    let tmp_path = tmp_dir.path().join("live-thumb.jpg");
    tokio::fs::write(&tmp_path, &body).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to write temp file");
        AppError::Internal("failed to write temp file".to_string())
    })?;

    let key = format!("streams/{}/live-thumb.jpg", stream_key);
    store.upload_file(&tmp_path, &key).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to upload thumbnail to storage");
        AppError::Internal("failed to upload thumbnail".to_string())
    })?;
    let thumbnail_url = store.public_url(&key);

    let active = stream::ActiveModel {
        id: Set(id),
        thumbnail_url: Set(Some(thumbnail_url.clone())),
        ..Default::default()
    };
    txn.stream_repo().update(active).await?;
    txn.commit().await?;

    Ok(Json(DataResponse {
        data: ThumbnailResponse { thumbnail_url },
    }))
}
