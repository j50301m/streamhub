use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use cache::CacheStore;
use chrono::Utc;
use entity::{stream, user};
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

fn live_stream(user_id: Uuid) -> stream::Model {
    let id = Uuid::new_v4();
    stream::Model {
        id,
        user_id: Some(user_id),
        stream_key: id.to_string(),
        title: Some("Test Stream".to_string()),
        status: stream::StreamStatus::Live,
        vod_status: stream::VodStatus::None,
        started_at: Some(Utc::now()),
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

fn test_rate_limiter() -> std::sync::Arc<dyn rate_limit::RateLimiter> {
    std::sync::Arc::new(rate_limit::InMemoryRateLimiter::new())
}

fn test_chat_policy() -> rate_limit::RateLimitPolicy {
    rate_limit::RateLimitPolicy {
        name: "chat".into(),
        limit: 1,
        window_secs: 1,
        key_prefix: "ratelimit:chat".into(),
    }
}

fn test_refresh_policy() -> rate_limit::RateLimitPolicy {
    rate_limit::RateLimitPolicy {
        name: "refresh".into(),
        limit: 10,
        window_secs: 60,
        key_prefix: "ratelimit:refresh".into(),
    }
}

fn app(state: AppState) -> axum::Router {
    let config = state.config.clone();
    let limiter = state.rate_limiter.clone();
    routes::app_router(limiter, &config).with_state(state)
}

/// Upload thumbnail success: thumbnail_url written back correctly, format
/// matches `streams/{stream_key}/live-thumb.jpg`.
#[tokio::test]
async fn upload_thumbnail_success() {
    let user = broadcaster_user();
    let s = live_stream(user.id);
    let stream_key = s.stream_key.clone();

    // After update, stream has the new thumbnail_url
    let mut updated = s.clone();
    updated.thumbnail_url = Some(format!(
        "http://mock-storage/streams/{}/live-thumb.jpg",
        stream_key
    ));

    // Mock DB calls (in order):
    // 1. access_state: find_by_id (user) — but we pre-set Redis cache to skip
    // 2. auth middleware: find_by_id (user)
    // 3. find_by_id_for_update (stream) — inside txn
    // 4. update (stream) — returns updated model
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // auth: find_by_id
        .append_query_results([vec![s.clone()]]) // find_by_id_for_update
        .append_query_results([vec![updated.clone()]]) // update returns
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .into_connection();

    // Pre-set access state in cache so we skip the DB lookup
    let cache = std::sync::Arc::new(cache::InMemoryCache::new());
    let access_key = mediamtx::keys::user_state(&user.id);
    cache.set(&access_key, "active", Some(300)).await.unwrap();

    let storage = super::test_storage();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: storage.clone(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache,
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let jpeg_bytes = b"\xFF\xD8\xFF\xE0fake-jpeg-payload";

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/streams/{}/thumbnail", s.id))
        .header("content-type", "image/jpeg")
        .header("authorization", auth_header(user.id))
        .body(Body::from(jpeg_bytes.to_vec()))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    let thumbnail_url = json["data"]["thumbnail_url"].as_str().unwrap();

    // Verify format: must match streams/{stream_key}/live-thumb.jpg
    assert_eq!(
        thumbnail_url,
        format!("http://mock-storage/streams/{}/live-thumb.jpg", stream_key)
    );
}

/// Thumbnail upload rejected when stream is not live or vod-ready.
#[tokio::test]
async fn upload_thumbnail_wrong_status() {
    let user = broadcaster_user();
    let id = Uuid::new_v4();
    let s = stream::Model {
        id,
        user_id: Some(user.id),
        stream_key: id.to_string(),
        title: Some("Pending Stream".to_string()),
        status: stream::StreamStatus::Pending,
        vod_status: stream::VodStatus::None,
        started_at: None,
        ended_at: None,
        created_at: Utc::now(),
        hls_url: None,
        thumbnail_url: None,
    };

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // auth: find_by_id
        .append_query_results([vec![s.clone()]]) // find_by_id_for_update
        .into_connection();

    let cache = std::sync::Arc::new(cache::InMemoryCache::new());
    let access_key = mediamtx::keys::user_state(&user.id);
    cache.set(&access_key, "active", Some(300)).await.unwrap();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache,
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/streams/{}/thumbnail", s.id))
        .header("content-type", "image/jpeg")
        .header("authorization", auth_header(user.id))
        .body(Body::from(b"fake-jpeg".to_vec()))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

/// Non-owner gets 403 Forbidden.
#[tokio::test]
async fn upload_thumbnail_not_owner() {
    let user = broadcaster_user();
    let other_user_id = Uuid::new_v4();
    let s = live_stream(other_user_id); // stream owned by someone else

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // auth: find_by_id
        .append_query_results([vec![s.clone()]]) // find_by_id_for_update
        .into_connection();

    let cache = std::sync::Arc::new(cache::InMemoryCache::new());
    let access_key = mediamtx::keys::user_state(&user.id);
    cache.set(&access_key, "active", Some(300)).await.unwrap();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache,
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/streams/{}/thumbnail", s.id))
        .header("content-type", "image/jpeg")
        .header("authorization", auth_header(user.id))
        .body(Body::from(b"fake-jpeg".to_vec()))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
