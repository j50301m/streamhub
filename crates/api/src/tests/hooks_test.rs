use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use common::AppState;
use entity::{recording, stream};
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
use tower::ServiceExt;
use uuid::Uuid;

use super::test_config;
use crate::routes;

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

fn ended_stream() -> stream::Model {
    let id = Uuid::new_v4();
    stream::Model {
        id,
        user_id: Some(Uuid::new_v4()),
        stream_key: id.to_string(),
        title: Some("Test".to_string()),
        status: stream::StreamStatus::Ended,
        vod_status: stream::VodStatus::None,
        started_at: Some(Utc::now()),
        ended_at: Some(Utc::now()),
        created_at: Utc::now(),
        hls_url: None,
    }
}

fn app(state: AppState) -> axum::Router {
    routes::app_router().with_state(state)
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
        storage: None,
    };

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

    let resp = app(state).oneshot(req).await.unwrap();
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
        storage: None,
    };

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

    let resp = app(state).oneshot(req).await.unwrap();
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
        storage: None,
    };

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

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recording_hook_ended_stream_triggers_transcode() {
    let s = ended_stream();
    let rec = recording::Model {
        id: Uuid::new_v4(),
        stream_id: s.id,
        file_path: "/recordings/test.mp4".to_string(),
        duration_secs: None,
        file_size_bytes: None,
        created_at: Utc::now(),
    };
    let mut updated = s.clone();
    updated.vod_status = stream::VodStatus::Processing;

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![s.clone()]]) // find_by_key_for_update
        .append_query_results([vec![rec.clone()]]) // create recording
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .append_query_results([vec![updated.clone()]]) // update vod_status
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        // For the background transcode task (find_latest_recording_by_stream)
        .append_query_results([vec![rec.clone()]])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: None,
    };

    let req = Request::builder()
        .method("POST")
        .uri("/internal/hooks/recording")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "stream_key": s.stream_key,
                "segment_path": "/recordings/test.mp4"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn recording_hook_live_stream_no_transcode() {
    let s = live_stream();
    let rec = recording::Model {
        id: Uuid::new_v4(),
        stream_id: s.id,
        file_path: "/recordings/test.mp4".to_string(),
        duration_secs: None,
        file_size_bytes: None,
        created_at: Utc::now(),
    };

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![s.clone()]]) // find_by_key_for_update
        .append_query_results([vec![rec.clone()]]) // create recording
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: None,
    };

    let req = Request::builder()
        .method("POST")
        .uri("/internal/hooks/recording")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "stream_key": s.stream_key,
                "segment_path": "/recordings/test.mp4"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
