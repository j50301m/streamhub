use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use entity::{stream, user};
use repo::UnitOfWork;
use sea_orm::prelude::*;
use sea_orm::{DbBackend, MockDatabase};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

use super::{JWT_SECRET, body_to_json, test_config};
use crate::routes;
use crate::state::BoAppState;

fn admin_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "admin@example.com".to_string(),
        password_hash: auth::password::hash_password("password123").unwrap(),
        role: user::UserRole::Admin,
        created_at: Utc::now(),
    }
}

fn broadcaster_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "broadcaster@example.com".to_string(),
        password_hash: auth::password::hash_password("password123").unwrap(),
        role: user::UserRole::Broadcaster,
        created_at: Utc::now(),
    }
}

fn make_token(user: &user::Model) -> String {
    auth::jwt::sign_access_token(user.id, JWT_SECRET).unwrap()
}

fn count_result(n: i64) -> Vec<Vec<BTreeMap<String, Value>>> {
    vec![vec![BTreeMap::from([(
        "num_items".to_string(),
        Value::BigInt(Some(n)),
    )])]]
}

fn app(state: BoAppState) -> axum::Router {
    routes::app_router().with_state(state)
}

#[tokio::test]
async fn admin_dashboard_returns_200_with_correct_shape() {
    let user = admin_user();
    let token = make_token(&user);

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]])
        .append_query_results(count_result(2))
        .append_query_results(count_result(1))
        .append_query_results(count_result(5))
        .append_query_results(count_result(100))
        .append_query_results(count_result(10))
        .append_query_results::<stream::Model, _, _>([vec![]])
        .into_connection();

    let state = BoAppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        cache: Arc::new(cache::InMemoryCache::new()),
    };

    let req = Request::builder()
        .method("GET")
        .uri("/v1/admin/dashboard")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    let data = &json["data"];
    assert_eq!(data["live_stream_count"], 2);
    assert_eq!(data["total_user_count"], 100);
    assert_eq!(data["broadcaster_count"], 10);
    assert_eq!(data["ended_streams_24h"], 5);
    assert_eq!(data["error_stream_count"], 1);
    assert!(data["recent_live_streams"].is_array());
    assert_eq!(data["recent_live_streams"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn admin_dashboard_returns_403_for_non_admin() {
    let user = broadcaster_user();
    let token = make_token(&user);

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]])
        .into_connection();

    let state = BoAppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        cache: Arc::new(cache::InMemoryCache::new()),
    };

    let req = Request::builder()
        .method("GET")
        .uri("/v1/admin/dashboard")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_dashboard_returns_401_without_token() {
    let db = MockDatabase::new(DbBackend::Postgres).into_connection();

    let state = BoAppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        cache: Arc::new(cache::InMemoryCache::new()),
    };

    let req = Request::builder()
        .method("GET")
        .uri("/v1/admin/dashboard")
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
