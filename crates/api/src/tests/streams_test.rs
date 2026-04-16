use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use cache::CacheStore;
use chrono::Utc;
use entity::{stream, user};
use mediamtx::MtxInstance;
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
use tower::ServiceExt;
use uuid::Uuid;

use super::{JWT_SECRET, body_to_json, test_config};
use crate::routes;

fn broadcaster_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "broadcaster@example.com".to_string(),
        password_hash: String::new(),
        role: user::UserRole::Broadcaster,
        is_suspended: false,
        suspended_until: None,
        suspension_reason: None,
        created_at: Utc::now(),
    }
}

fn viewer_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "viewer@example.com".to_string(),
        password_hash: String::new(),
        role: user::UserRole::Viewer,
        is_suspended: false,
        suspended_until: None,
        suspension_reason: None,
        created_at: Utc::now(),
    }
}

fn test_stream(user_id: Uuid) -> stream::Model {
    let id = Uuid::new_v4();
    stream::Model {
        id,
        user_id: Some(user_id),
        stream_key: id.to_string(),
        title: Some("Test Stream".to_string()),
        status: stream::StreamStatus::Pending,
        vod_status: stream::VodStatus::None,
        started_at: None,
        ended_at: None,
        created_at: Utc::now(),
        hls_url: None,
        thumbnail_url: None,
    }
}

fn auth_header(user_id: Uuid) -> String {
    let token = auth::jwt::sign_access_token(user_id, JWT_SECRET).unwrap();
    format!("Bearer {token}")
}

fn app(state: AppState) -> axum::Router {
    routes::app_router().with_state(state)
}

fn test_mtx_instance() -> MtxInstance {
    MtxInstance {
        name: "mtx-1".to_string(),
        internal_api: "http://mtx-1:9997".to_string(),
        public_whip: "http://localhost:8889".to_string(),
        public_whep: "http://localhost:8889".to_string(),
        public_hls: "http://localhost:8888".to_string(),
    }
}

#[tokio::test]
async fn create_stream_success() {
    let user = broadcaster_user();
    let s = test_stream(user.id);

    // Mock: access_state find_by_id, auth middleware find_by_id, then txn create
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // access_state: find_by_id
        .append_query_results([vec![user.clone()]]) // auth: find_by_id
        .append_query_results([vec![s.clone()]]) // create stream
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
        .uri("/v1/streams")
        .header("content-type", "application/json")
        .header("authorization", auth_header(user.id))
        .body(Body::from(r#"{"title":"My Stream"}"#))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_to_json(resp.into_body()).await;
    assert_eq!(json["data"]["title"], "Test Stream");
}

#[tokio::test]
async fn create_stream_viewer_forbidden() {
    let user = viewer_user();

    // Mock: access_state find_by_id, auth middleware find_by_id
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // access_state: find_by_id
        .append_query_results([vec![user.clone()]]) // auth: find_by_id
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
        .uri("/v1/streams")
        .header("content-type", "application/json")
        .header("authorization", auth_header(user.id))
        .body(Body::from(r#"{"title":"My Stream"}"#))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_stream_token_returns_409_when_stream_was_force_ended() {
    let user = broadcaster_user();
    let s = test_stream(user.id);
    let cache = std::sync::Arc::new(cache::InMemoryCache::new());
    let mtx = test_mtx_instance();
    cache
        .set(&mediamtx::keys::mtx_status(&mtx.name), "healthy", None)
        .await
        .unwrap();
    cache
        .set(&mediamtx::keys::stream_force_ended(&s.id), "1", None)
        .await
        .unwrap();

    // access_state: find_by_id, auth middleware find_by_id, stream find_by_id
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![s.clone()]])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache,
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![mtx],
    };

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/streams/{}/token", s.id))
        .header("authorization", auth_header(user.id))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let json = body_to_json(resp.into_body()).await;
    assert_eq!(json["error"]["message"], "Conflict: STREAM_FORCE_ENDED");
}
