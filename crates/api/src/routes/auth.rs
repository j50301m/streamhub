use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use common::{AppError, AppState};
use entity::user;
use sea_orm::Set;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::middleware::CurrentUser;

// ── Request / Response types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub user: UserResponse,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

#[derive(Debug, Serialize)]
struct DataResponse<T: Serialize> {
    data: T,
}

fn user_to_response(model: &user::Model) -> UserResponse {
    UserResponse {
        id: model.id,
        email: model.email.clone(),
        role: role_to_string(&model.role),
        created_at: model.created_at,
    }
}

fn role_to_string(role: &user::UserRole) -> String {
    match role {
        user::UserRole::Broadcaster => "broadcaster".to_string(),
        user::UserRole::Viewer => "viewer".to_string(),
        user::UserRole::Admin => "admin".to_string(),
    }
}

fn parse_role(s: &str) -> Result<user::UserRole, AppError> {
    match s.to_lowercase().as_str() {
        "broadcaster" => Ok(user::UserRole::Broadcaster),
        "viewer" => Ok(user::UserRole::Viewer),
        _ => Err(AppError::Validation(
            "role must be 'broadcaster' or 'viewer'".to_string(),
        )),
    }
}

// ── Handlers ───────────────────────────────────────────────────────

/// POST /v1/auth/register
async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<DataResponse<AuthResponse>>), AppError> {
    // Validate
    let email = payload.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::Validation(
            "email must be a valid email address".to_string(),
        ));
    }
    if payload.password.len() < 8 {
        return Err(AppError::Validation(
            "password must be at least 8 characters".to_string(),
        ));
    }
    let role = parse_role(&payload.role)?;

    // Use transaction with FOR UPDATE to prevent concurrent registration
    let txn = state.uow.begin().await?;

    let existing = txn.user_repo().find_by_email_for_update(&email).await?;
    if existing.is_some() {
        return Err(AppError::Conflict("USER_ALREADY_EXISTS".to_string()));
    }

    // Hash password
    let password_hash = auth::password::hash_password(&payload.password)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Create user
    let user_id = Uuid::new_v4();
    let active = user::ActiveModel {
        id: Set(user_id),
        email: Set(email),
        password_hash: Set(password_hash),
        role: Set(role),
        created_at: Set(Utc::now()),
    };
    let model = txn.user_repo().create(active).await?;

    txn.commit().await?;

    // Generate tokens
    let access_token = auth::jwt::sign_access_token(model.id, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let refresh_token = auth::jwt::sign_refresh_token(model.id, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let resp = AuthResponse {
        user: user_to_response(&model),
        access_token,
        refresh_token,
        expires_in: auth::jwt::access_token_expires_in(),
    };

    Ok((StatusCode::CREATED, Json(DataResponse { data: resp })))
}

/// POST /v1/auth/login
async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<DataResponse<AuthResponse>>, AppError> {
    let email = payload.email.trim().to_lowercase();

    let model = state
        .uow
        .user_repo()
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::Unauthorized("INVALID_CREDENTIALS".to_string()))?;

    auth::password::verify_password(&payload.password, &model.password_hash)
        .map_err(|_| AppError::Unauthorized("INVALID_CREDENTIALS".to_string()))?;

    let access_token = auth::jwt::sign_access_token(model.id, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let refresh_token = auth::jwt::sign_refresh_token(model.id, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let resp = AuthResponse {
        user: user_to_response(&model),
        access_token,
        refresh_token,
        expires_in: auth::jwt::access_token_expires_in(),
    };

    Ok(Json(DataResponse { data: resp }))
}

/// POST /v1/auth/refresh
async fn refresh(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Json<DataResponse<TokenResponse>>, AppError> {
    let claims = auth::jwt::verify_token(&payload.refresh_token, &state.config.jwt_secret)
        .map_err(|e| match e {
            auth::jwt::JwtError::Expired => {
                AppError::Unauthorized("REFRESH_TOKEN_INVALID".to_string())
            }
            auth::jwt::JwtError::Invalid => {
                AppError::Unauthorized("REFRESH_TOKEN_INVALID".to_string())
            }
        })?;

    if claims.typ != "refresh" {
        return Err(AppError::Unauthorized("REFRESH_TOKEN_INVALID".to_string()));
    }

    // Verify user still exists
    state
        .uow
        .user_repo()
        .find_by_id(claims.sub)
        .await?
        .ok_or_else(|| AppError::Unauthorized("REFRESH_TOKEN_INVALID".to_string()))?;

    let access_token = auth::jwt::sign_access_token(claims.sub, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let new_refresh = auth::jwt::sign_refresh_token(claims.sub, &state.config.jwt_secret)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(DataResponse {
        data: TokenResponse {
            access_token,
            refresh_token: new_refresh,
            expires_in: auth::jwt::access_token_expires_in(),
        },
    }))
}

/// POST /v1/auth/logout
/// Since we use stateless JWTs, logout is a no-op on the server side.
/// The client should discard the tokens.
async fn logout(_current_user: CurrentUser, Json(_payload): Json<LogoutRequest>) -> StatusCode {
    StatusCode::NO_CONTENT
}

/// GET /v1/me
async fn me(
    current_user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<DataResponse<UserResponse>>, AppError> {
    let model = state
        .uow
        .user_repo()
        .find_by_id(current_user.id)
        .await?
        .ok_or_else(|| AppError::NotFound("USER_NOT_FOUND".to_string()))?;

    Ok(Json(DataResponse {
        data: user_to_response(&model),
    }))
}

pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/refresh", post(refresh))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/me", get(me))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use common::AppConfig;
    use http_body_util::BodyExt;
    use repo::UnitOfWork;
    use sea_orm::{DbBackend, MockDatabase, MockExecResult};
    use tower::ServiceExt;

    const JWT_SECRET: &str = "test-secret";

    fn test_config() -> AppConfig {
        AppConfig {
            database_url: String::new(),
            host: "127.0.0.1".to_string(),
            port: 0,
            mediamtx_url: "http://localhost:9997".to_string(),
            jwt_secret: JWT_SECRET.to_string(),
            recordings_path: "/tmp/recordings".to_string(),
        }
    }

    fn test_user() -> user::Model {
        user::Model {
            id: Uuid::new_v4(),
            email: "test@example.com".to_string(),
            password_hash: auth::password::hash_password("password123").unwrap(),
            role: user::UserRole::Broadcaster,
            created_at: Utc::now(),
        }
    }

    async fn body_to_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn register_success() {
        let user = test_user();
        // Mock: find_by_email_for_update returns None (no existing user), then create returns user
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
        };

        let app = auth_routes().with_state(state);

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

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let json = body_to_json(resp.into_body()).await;
        assert!(json["data"]["access_token"].is_string());
        assert!(json["data"]["user"]["email"].as_str().unwrap() == "test@example.com");
    }

    #[tokio::test]
    async fn register_duplicate_email_returns_409() {
        let user = test_user();
        // Mock: find_by_email_for_update returns existing user
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user.clone()]]) // find_by_email_for_update: found
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = auth_routes().with_state(state);

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

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn login_success() {
        let user = test_user();
        // Mock: find_by_email returns user
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user.clone()]]) // find_by_email
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = auth_routes().with_state(state);

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

        let resp = app.oneshot(req).await.unwrap();
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
        };

        let app = auth_routes().with_state(state);

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

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_user_not_found_returns_401() {
        // Mock: find_by_email returns empty
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results::<user::Model, _, _>([vec![]])
            .into_connection();

        let state = AppState {
            uow: UnitOfWork::new(db),
            config: test_config(),
        };

        let app = auth_routes().with_state(state);

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

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
