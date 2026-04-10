use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use common::AppState;
use entity::stream;
use sea_orm::Set;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PublishHookPayload {
    pub stream_key: String,
    pub action: String,
}

/// POST /internal/hooks/publish
/// Called by MediaMTX on publish/unpublish events.
pub async fn publish_hook(
    State(state): State<AppState>,
    Json(payload): Json<PublishHookPayload>,
) -> Result<StatusCode, StatusCode> {
    tracing::info!(
        stream_key = %payload.stream_key,
        action = %payload.action,
        "Received publish hook"
    );

    let txn = state.uow.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let stream = txn
        .stream_repo()
        .find_by_key_for_update(&payload.stream_key)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let stream = match stream {
        Some(s) => s,
        None => {
            tracing::warn!(stream_key = %payload.stream_key, "Stream not found for hook");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let mut active: stream::ActiveModel = stream.into();

    match payload.action.as_str() {
        "publish" => {
            active.status = Set(stream::StreamStatus::Live);
            active.started_at = Set(Some(Utc::now()));
        }
        "unpublish" => {
            active.status = Set(stream::StreamStatus::Ended);
            active.ended_at = Set(Some(Utc::now()));
        }
        _ => {
            tracing::warn!(action = %payload.action, "Unknown hook action");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    txn.stream_repo().update(active).await.map_err(|e| {
        tracing::error!("Failed to update stream: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use common::{AppConfig, AppState};
    use repo::UnitOfWork;
    use sea_orm::{DbBackend, MockDatabase, MockExecResult};
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> AppConfig {
        AppConfig {
            database_url: String::new(),
            host: "127.0.0.1".to_string(),
            port: 0,
            mediamtx_url: "http://localhost:9997".to_string(),
            jwt_secret: "test-secret".to_string(),
            recordings_path: "/tmp/recordings".to_string(),
        }
    }

    fn pending_stream() -> stream::Model {
        let id = Uuid::new_v4();
        stream::Model {
            id,
            user_id: Some(Uuid::new_v4()),
            stream_key: id.to_string(),
            title: Some("Test".to_string()),
            status: stream::StreamStatus::Pending,
            vod_status: stream::VodStatus::None,
            started_at: None,
            ended_at: None,
            created_at: Utc::now(),
            hls_url: None,
        }
    }

    fn live_stream() -> stream::Model {
        let mut s = pending_stream();
        s.status = stream::StreamStatus::Live;
        s.started_at = Some(Utc::now());
        s
    }

    #[tokio::test]
    async fn publish_sets_status_to_live() {
        let s = pending_stream();
        let mut updated = s.clone();
        updated.status = stream::StreamStatus::Live;
        updated.started_at = Some(Utc::now());

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]]) // find_by_key_for_update
            .append_query_results([vec![updated.clone()]]) // update
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = Router::new()
            .route("/internal/hooks/publish", post(publish_hook))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": s.stream_key,
                    "action": "publish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unpublish_sets_status_to_ended() {
        let s = live_stream();
        let mut updated = s.clone();
        updated.status = stream::StreamStatus::Ended;
        updated.ended_at = Some(Utc::now());

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![s.clone()]])
            .append_query_results([vec![updated.clone()]])
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = Router::new()
            .route("/internal/hooks/publish", post(publish_hook))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": s.stream_key,
                    "action": "unpublish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn publish_stream_not_found_returns_404() {
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results::<stream::Model, _, _>([vec![]]) // find: empty
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = Router::new()
            .route("/internal/hooks/publish", post(publish_hook))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/internal/hooks/publish")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "stream_key": "nonexistent-key",
                    "action": "publish"
                }))
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
