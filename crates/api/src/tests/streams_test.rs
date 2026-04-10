use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use common::AppState;
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
        created_at: Utc::now(),
    }
}

fn viewer_user() -> user::Model {
    user::Model {
        id: Uuid::new_v4(),
        email: "viewer@example.com".to_string(),
        password_hash: String::new(),
        role: user::UserRole::Viewer,
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
    }
}

fn auth_header(user_id: Uuid) -> String {
    let token = auth::jwt::sign_access_token(user_id, JWT_SECRET).unwrap();
    format!("Bearer {token}")
}

fn app(state: AppState) -> axum::Router {
    routes::app_router().with_state(state)
}

#[tokio::test]
async fn create_stream_success() {
    let user = broadcaster_user();
    let s = test_stream(user.id);

    // Mock: auth middleware find_by_id, then txn create
    let db = MockDatabase::new(DbBackend::Postgres)
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

    // Mock: auth middleware find_by_id
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([vec![user.clone()]]) // auth: find_by_id
        .into_connection();

    let state = AppState {
        uow: UnitOfWork::new(db),
        config: test_config(),
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
