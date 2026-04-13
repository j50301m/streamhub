use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use common::AppState;
use entity::user;
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
use tower::ServiceExt;
use uuid::Uuid;

use super::{JWT_SECRET, body_to_json, test_config};
use crate::routes;

fn test_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "test@example.com".to_string(),
        password_hash: auth::password::hash_password("password123").unwrap(),
        role: user::UserRole::Broadcaster,
        created_at: Utc::now(),
    }
}

fn app(state: AppState) -> axum::Router {
    routes::app_router().with_state(state)
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
        storage: None,
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        live_tasks: Default::default(),
        mtx_instances: vec![],
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
        storage: None,
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        live_tasks: Default::default(),
        mtx_instances: vec![],
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
        storage: None,
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        live_tasks: Default::default(),
        mtx_instances: vec![],
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
        storage: None,
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        live_tasks: Default::default(),
        mtx_instances: vec![],
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
        storage: None,
        metrics: super::test_metrics(),
        redis_pool: super::test_redis_pool(),
        cache: std::sync::Arc::new(cache::InMemoryCache::new()),
        live_tasks: Default::default(),
        mtx_instances: vec![],
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
