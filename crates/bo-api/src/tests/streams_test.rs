use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use entity::{stream, user};
use repo::UnitOfWork;
use sea_orm::prelude::*;
use sea_orm::{DbBackend, MockDatabase, MockExecResult};
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

fn test_state(db: sea_orm::DatabaseConnection) -> BoAppState {
    BoAppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
        cache: Arc::new(cache::InMemoryCache::new()),
        pubsub: Arc::new(cache::InMemoryPubSub::new()),
    }
}

fn live_stream(user_id: Uuid) -> stream::Model {
    stream::Model {
        id: Uuid::new_v4(),
        user_id: Some(user_id),
        stream_key: Uuid::new_v4().to_string(),
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

fn ended_stream(user_id: Uuid) -> stream::Model {
    let mut s = live_stream(user_id);
    s.status = stream::StreamStatus::Ended;
    s.ended_at = Some(Utc::now());
    s
}

// ── GET /v1/admin/streams ─────────────────────────────────────────

#[tokio::test]
async fn list_streams_returns_200() {
    let admin = admin_user();
    let owner = user::Model {
        id: Uuid::new_v4(),
        email: "owner@example.com".to_string(),
        password_hash: "hash".to_string(),
        role: user::UserRole::Broadcaster,
        is_suspended: false,
        suspended_until: None,
        suspension_reason: None,
        created_at: Utc::now(),
    };
    let token = make_token(&admin);
    let s = live_stream(owner.id);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // count for pagination
        .append_query_results(count_result(1))
        // paginated streams
        .append_query_results([vec![s.clone()]])
        // owner lookup
        .append_query_results([vec![owner.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/admin/streams?page=1&per_page=20")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    let data = &json["data"];
    assert_eq!(data["total"], 1);
    assert_eq!(data["page"], 1);
    assert_eq!(data["streams"].as_array().unwrap().len(), 1);
    assert_eq!(data["streams"][0]["owner_email"], "owner@example.com");
}

// ── GET /v1/admin/streams/:id ─────────────────────────────────────

#[tokio::test]
async fn stream_detail_returns_200() {
    let admin = admin_user();
    let token = make_token(&admin);
    let s = live_stream(admin.id);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // stream find_by_id
        .append_query_results([vec![s.clone()]])
        // owner lookup
        .append_query_results([vec![admin.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/admin/streams/{}", s.id))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    assert_eq!(json["data"]["id"], s.id.to_string());
    assert_eq!(json["data"]["status"], "live");
}

#[tokio::test]
async fn stream_detail_returns_404_for_missing() {
    let admin = admin_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![admin.clone()]])
        .append_query_results([vec![admin.clone()]])
        .append_query_results::<stream::Model, _, _>([vec![]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/admin/streams/{}", Uuid::new_v4()))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── POST /v1/admin/streams/:id/end ────────────────────────────────

#[tokio::test]
async fn force_end_returns_200_for_live_stream() {
    let admin = admin_user();
    let token = make_token(&admin);
    let s = live_stream(admin.id);
    let mut ended = s.clone();
    ended.status = stream::StreamStatus::Ended;
    ended.ended_at = Some(Utc::now());

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // stream find_by_id
        .append_query_results([vec![s.clone()]])
        // stream update
        .append_query_results([vec![ended.clone()]])
        .append_exec_results([MockExecResult {
            last_insert_id: 0,
            rows_affected: 1,
        }])
        // owner lookup
        .append_query_results([vec![admin.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/admin/streams/{}/end", s.id))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_to_json(resp.into_body()).await;
    assert_eq!(json["data"]["status"], "ended");
}

#[tokio::test]
async fn force_end_returns_409_for_non_live_stream() {
    let admin = admin_user();
    let token = make_token(&admin);
    let s = ended_stream(admin.id);

    let db = MockDatabase::new(DbBackend::Postgres)
        // access_state: find_by_id
        .append_query_results([vec![admin.clone()]])
        // extractor: find_by_id
        .append_query_results([vec![admin.clone()]])
        // stream find_by_id (ended)
        .append_query_results([vec![s.clone()]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/admin/streams/{}/end", s.id))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn force_end_returns_404_for_missing_stream() {
    let admin = admin_user();
    let token = make_token(&admin);

    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![admin.clone()]])
        .append_query_results([vec![admin.clone()]])
        .append_query_results::<stream::Model, _, _>([vec![]])
        .into_connection();

    let state = test_state(db);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/admin/streams/{}/end", Uuid::new_v4()))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
