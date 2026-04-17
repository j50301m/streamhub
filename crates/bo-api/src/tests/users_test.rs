use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use entity::user;
use repo::UnitOfWork;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
use std::collections::BTreeMap;
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
        is_suspended: false,
        suspended_until: None,
        suspension_reason: None,
        created_at: Utc::now(),
    }
}

fn target_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "target@example.com".to_string(),
        password_hash: auth::password::hash_password("password123").unwrap(),
        role: user::UserRole::Viewer,
        is_suspended: false,
        suspended_until: None,
        suspension_reason: None,
        created_at: Utc::now(),
    }
}

fn suspended_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "suspended@example.com".to_string(),
        password_hash: auth::password::hash_password("password123").unwrap(),
        role: user::UserRole::Admin,
        is_suspended: true,
        suspended_until: None,
        suspension_reason: Some("spam".to_string()),
        created_at: Utc::now(),
    }
}

fn make_token(user: &user::Model) -> String {
    auth::jwt::sign_access_token(user.id, JWT_SECRET).unwrap()
}

fn count_result(n: i64) -> Vec<Vec<BTreeMap<String, sea_orm::prelude::Value>>> {
    vec![vec![BTreeMap::from([(
        "num_items".to_string(),
        sea_orm::prelude::Value::BigInt(Some(n)),
    )])]]
}

fn app(state: BoAppState) -> axum::Router {
    routes::app_router().with_state(state)
}

fn test_state(db: sea_orm::DatabaseConnection) -> BoAppState {
    BoAppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        cache: Arc::new(cache::InMemoryCache::new()),
        pubsub: Arc::new(cache::InMemoryPubSub::new()),
        metrics: super::test_metrics(),
    }
}

// ── GET /v1/admin/users ────────────────────────────────────────────

#[tokio::test]
async fn list_users_returns_200_with_correct_shape() {
    let admin = admin_user();
    let target = target_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // count for pagination
        .append_query_results(count_result(1))
        // paginated users
        .append_query_results([vec![target.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/admin/users?page=1&per_page=20")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    let data = &json["data"];
    assert!(data["users"].is_array());
    assert_eq!(data["total"], 1);
    assert_eq!(data["page"], 1);
    assert_eq!(data["per_page"], 20);
    let users = data["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["email"], "target@example.com");
}

// ── PATCH /v1/admin/users/:id/role ─────────────────────────────────

#[tokio::test]
async fn update_role_success() {
    let admin = admin_user();
    let target = target_user();
    let token = make_token(&admin);

    let mut updated_target = target.clone();
    updated_target.role = user::UserRole::Broadcaster;

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // update_role: find_by_id
        .append_query_results([vec![target.clone()]])
        // update_role: update result
        .append_query_results([vec![updated_target.clone()]])
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/admin/users/{}/role", target.id))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"role":"broadcaster"}"#))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_role_self_returns_400() {
    let admin = admin_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/admin/users/{}/role", admin.id))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"role":"viewer"}"#))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── POST /v1/admin/users/:id/suspend ───────────────────────────────

#[tokio::test]
async fn suspend_user_returns_200() {
    let admin = admin_user();
    let target = target_user();
    let token = make_token(&admin);

    let mut suspended_target = target.clone();
    suspended_target.is_suspended = true;
    suspended_target.suspension_reason = Some("spam".to_string());

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // set_suspended: find_by_id
        .append_query_results([vec![target.clone()]])
        // set_suspended: update result
        .append_query_results([vec![suspended_target.clone()]])
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/admin/users/{}/suspend", target.id))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"duration_secs":86400,"reason":"spam"}"#))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    assert_eq!(json["data"]["is_suspended"], true);
}

#[tokio::test]
async fn suspend_self_returns_400() {
    let admin = admin_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/admin/users/{}/suspend", admin.id))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"duration_secs":null,"reason":"test"}"#))
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── DELETE /v1/admin/users/:id/suspend ─────────────────────────────

#[tokio::test]
async fn unsuspend_user_returns_200() {
    let admin = admin_user();
    let target = target_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // clear_suspended: find_by_id
        .append_query_results([vec![target.clone()]])
        // clear_suspended: update result
        .append_query_results([vec![target.clone()]])
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/admin/users/{}/suspend", target.id))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Suspended admin cannot access endpoints ────────────────────────

#[tokio::test]
async fn suspended_admin_gets_403() {
    let admin = suspended_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id → returns suspended user
        .append_query_results([vec![admin.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/admin/users")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
