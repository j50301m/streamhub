use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use entity::user;
use rate_limit::{RateLimitLayer, RateLimitMode, RateLimitPolicy};
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
use tower::ServiceExt;
use uuid::Uuid;

use super::{JWT_SECRET, body_to_json, test_config};
use crate::{middleware, routes};

fn test_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "test@example.com".to_string(),
        password_hash: auth::password::hash_password("password123").unwrap(),
        role: user::UserRole::Broadcaster,
        is_suspended: false,
        suspended_until: None,
        suspension_reason: None,
        created_at: Utc::now(),
    }
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

fn app_with_general_limits(state: AppState) -> axum::Router {
    let config = state.config.clone();
    let limiter = state.rate_limiter.clone();
    let general_unauthed_policy = RateLimitPolicy {
        name: "api_general_unauthed".into(),
        limit: config.rate_limit_general_unauthed_limit,
        window_secs: config.rate_limit_general_unauthed_window,
        key_prefix: "ratelimit:api_general_unauthed".into(),
    };
    let general_authed_policy = RateLimitPolicy {
        name: "api_general_authed".into(),
        limit: config.rate_limit_general_authed_limit,
        window_secs: config.rate_limit_general_authed_window,
        key_prefix: "ratelimit:api_general_authed".into(),
    };

    axum::Router::new()
        .merge(routes::app_router(limiter.clone(), &config))
        .layer(RateLimitLayer::new(
            limiter.clone(),
            general_authed_policy,
            RateLimitMode::UserIdOnly,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (limiter, general_unauthed_policy),
            middleware::rate_limit::unauthed_rate_limit,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::inject_user_id_extension,
        ))
        .with_state(state)
}

#[tokio::test]
async fn register_success() {
    let user = test_user();
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results::<user::Model, _, _>([vec![]]) // find_by_email_for_update: empty
        .append_query_results([vec![user.clone()]]) // create: inserted user
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
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/register")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "email": "test@example.com",
                "password": "password123",
                "role": "broadcaster"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_to_json(resp.into_body()).await;
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["user"]["email"].as_str().unwrap() == "test@example.com");
}

#[tokio::test]
async fn register_duplicate_email_returns_409() {
    let user = test_user();
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // find_by_email_for_update: found
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
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/register")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "email": "test@example.com",
                "password": "password123",
                "role": "broadcaster"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn login_success() {
    let user = test_user();
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // find_by_email
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
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "email": "test@example.com",
                "password": "password123"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    assert!(json["data"]["access_token"].is_string());
}

#[tokio::test]
async fn login_wrong_password_returns_401() {
    let user = test_user();
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]])
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
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "email": "test@example.com",
                "password": "wrongpassword"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_user_not_found_returns_401() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results::<user::Model, _, _>([vec![]])
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
        rate_limiter: test_rate_limiter(),
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let _ = JWT_SECRET; // suppress unused warning

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "email": "noone@example.com",
                "password": "password123"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_is_rate_limited_per_user() {
    let user = test_user();
    let refresh_token = auth::jwt::sign_refresh_token(user.id, JWT_SECRET).unwrap();
    let limiter = test_rate_limiter();
    let mut config = test_config();
    config.rate_limit_refresh_limit = 1;

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config,
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
        rate_limiter: limiter,
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: rate_limit::RateLimitPolicy {
            name: "refresh".into(),
            limit: 1,
            window_secs: 60,
            key_prefix: "ratelimit:refresh".into(),
        },
    };

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/refresh")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "refresh_token": refresh_token
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/refresh")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "refresh_token": refresh_token
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn authed_requests_do_not_consume_unauthed_ip_budget() {
    let user = test_user();
    let limiter = test_rate_limiter();
    let mut config = test_config();
    config.rate_limit_general_unauthed_limit = 1;
    config.rate_limit_general_authed_limit = 10;

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![user.clone()]])
        .append_query_results([vec![user.clone()]])
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config,
        storage: super::test_storage(),
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        pubsub: super::test_pubsub(),
        live_tasks: Default::default(),
        mtx_instances: vec![],
        rate_limiter: limiter,
        chat_rate_limit_policy: test_chat_policy(),
        refresh_rate_limit_policy: test_refresh_policy(),
    };

    let req = Request::builder()
        .method("GET")
        .uri("/v1/me")
        .header(
            "authorization",
            format!(
                "Bearer {}",
                auth::jwt::sign_access_token(user.id, JWT_SECRET).unwrap()
            ),
        )
        .header("x-forwarded-for", "1.2.3.4")
        .body(Body::empty())
        .unwrap();
    let resp = app_with_general_limits(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/me")
        .header(
            "authorization",
            format!(
                "Bearer {}",
                auth::jwt::sign_access_token(user.id, JWT_SECRET).unwrap()
            ),
        )
        .header("x-forwarded-for", "1.2.3.4")
        .body(Body::empty())
        .unwrap();
    let resp = app_with_general_limits(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
