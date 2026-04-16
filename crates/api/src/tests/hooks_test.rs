use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use entity::{recording, stream};
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
use tower::ServiceExt;
use uuid::Uuid;

use super::test_config;
use crate::routes;

fn live_stream() -> stream::Model {
    let id = Uuid::new_v4();
    stream::Model {
        id,
        user_id: Some(Uuid::new_v4()),
        stream_key: id.to_string(),
        title: Some("Test".to_string()),
        status: stream::StreamStatus::Live,
        vod_status: stream::VodStatus::None,
        started_at: Some(Utc::now()),
        ended_at: None,
        created_at: Utc::now(),
        hls_url: None,
        thumbnail_url: None,
    }
}

fn app(state: AppState) -> axum::Router {
    routes::app_router().with_state(state)
}

#[tokio::test]
async fn recording_hook_saves_segment() {
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
        .append_query_results([vec![s.clone()]]) // find_by_key
        .append_query_results([vec![rec.clone()]]) // create recording
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
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
async fn recording_hook_stream_not_found_returns_404() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results::<stream::Model, _, _>([vec![]])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
    };

    let req = Request::builder()
        .method("POST")
        .uri("/internal/hooks/recording")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "stream_key": "nonexistent",
                "segment_path": "/recordings/test.mp4"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
