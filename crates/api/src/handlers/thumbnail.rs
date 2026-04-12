use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use common::{AppError, AppState};
use entity::stream;
use sea_orm::Set;
use serde::Serialize;
use std::path::PathBuf;
use uuid::Uuid;

use crate::middleware::CurrentUser;

use super::streams::DataResponse;

#[derive(Debug, Serialize)]
pub struct ThumbnailResponse {
    pub thumbnail_url: String,
}

/// POST /v1/streams/:id/thumbnail
/// Accepts raw JPEG binary body. Owner only, stream must be Live or VodReady.
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
    let thumbnail_url = if let Some(store) = &state.storage {
        // GCS mode: upload bytes to storage
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
        store.public_url(&key)
    } else {
        // Local mode: write to thumbnails directory
        let thumb_dir = PathBuf::from(&state.config.thumbnails_path).join(stream_key);
        tokio::fs::create_dir_all(&thumb_dir).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to create thumbnail dir");
            AppError::Internal("failed to create thumbnail dir".to_string())
        })?;
        let thumb_path = thumb_dir.join("live-thumb.jpg");
        tokio::fs::write(&thumb_path, &body).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to write thumbnail file");
            AppError::Internal("failed to write thumbnail".to_string())
        })?;
        format!("/thumbnails/{}/live-thumb.jpg", stream_key)
    };

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
