use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use entity::user;
use error::AppError;
use sea_orm::Set;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::extractors::AppJson;
use crate::middleware::CurrentUser;

// ── Request / Response types ───────────────────────────────────────

/// Request body for `POST /v1/auth/register`.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// Email address (normalized to lowercase server-side).
    pub email: String,
    /// Plaintext password; minimum 8 characters.
    pub password: String,
    /// Desired role. `admin` is rejected to prevent self-elevation.
    pub role: user::UserRole,
}

/// Request body for `POST /v1/auth/login`.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// Email address.
    pub email: String,
    /// Plaintext password.
    pub password: String,
}

/// Public user representation returned by auth endpoints and `/v1/me`.
#[derive(Debug, Serialize)]
pub struct UserResponse {
    /// User UUID.
    pub id: Uuid,
    /// Email (lowercase).
    pub email: String,
    /// User role.
    pub role: user::UserRole,
    /// Account creation timestamp.
    pub created_at: chrono::DateTime<Utc>,
}

/// Request body for `POST /v1/auth/refresh`.
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    /// Previously issued refresh token.
    pub refresh_token: String,
}

/// Request body for `POST /v1/auth/logout`.
///
/// Accepted for future token-revocation support; currently no-op.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LogoutRequest {
    /// Refresh token to invalidate (reserved).
    pub refresh_token: String,
}

/// Response for successful register / login — user profile plus both JWTs.
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    /// Authenticated user profile.
    pub user: UserResponse,
    /// Short-lived JWT used as `Authorization: Bearer`.
    pub access_token: String,
    /// Long-lived JWT used only against `/v1/auth/refresh`.
    pub refresh_token: String,
    /// Access-token lifetime in seconds.
    pub expires_in: i64,
}

/// Response for `POST /v1/auth/refresh` — rotated tokens without user data.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// New short-lived access token.
    pub access_token: String,
    /// New long-lived refresh token (rotated on every refresh).
    pub refresh_token: String,
    /// Access-token lifetime in seconds.
    pub expires_in: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct DataResponse<T: Serialize> {
    pub(crate) data: T,
}

fn user_to_response(model: &user::Model) -> UserResponse {
    UserResponse {
        id: model.id,
        email: model.email.clone(),
        role: model.role.clone(),
        created_at: model.created_at,
    }
}

// ── Handlers ───────────────────────────────────────────────────────

/// `POST /v1/auth/register` — create a new user and issue initial tokens.
///
/// Returns 201 with `AuthResponse`.
///
/// # Errors
/// - 400 `Validation` for invalid email / short password / invalid role
/// - 409 `USER_ALREADY_EXISTS`
/// - 500 on password hashing / JWT signing / DB failure
#[tracing::instrument(skip(state, payload), fields(email = %payload.email))]
pub(crate) async fn register(
    State(state): State<AppState>,
    AppJson(payload): AppJson<RegisterRequest>,
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
    // Block admin self-registration
    if payload.role == user::UserRole::Admin {
        return Err(AppError::Validation(
            "role must be 'broadcaster' or 'viewer'".to_string(),
        ));
    }

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
        role: Set(payload.role),
        is_suspended: Set(false),
        suspended_until: Set(None),
        suspension_reason: Set(None),
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

/// `POST /v1/auth/login` — verify credentials and issue access + refresh tokens.
///
/// # Errors
/// - 401 `INVALID_CREDENTIALS` on unknown email or wrong password
/// - 500 on JWT signing / DB failure
#[tracing::instrument(skip(state, payload), fields(email = %payload.email))]
pub(crate) async fn login(
    State(state): State<AppState>,
    AppJson(payload): AppJson<LoginRequest>,
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

    // Suspended check with lazy expiration clearing
    let model = if model.is_suspended {
        if let Some(until) = model.suspended_until {
            if Utc::now() >= until {
                // Expired → lazy clear
                state.uow.user_repo().clear_suspended(model.id).await?;
                let key = mediamtx::keys::user_state(&model.id);
                let _ = state.cache.set(&key, "active", Some(300)).await;
                state
                    .uow
                    .user_repo()
                    .find_by_id(model.id)
                    .await?
                    .ok_or_else(|| AppError::Internal("user disappeared".to_string()))?
            } else {
                return Err(AppError::Forbidden(format!(
                    "Account suspended until {}",
                    until.format("%Y-%m-%d %H:%M UTC")
                )));
            }
        } else {
            return Err(AppError::Forbidden("Account suspended".to_string()));
        }
    } else {
        model
    };

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

/// `POST /v1/auth/refresh` — exchange a valid refresh token for a new pair.
///
/// # Errors
/// - 401 `REFRESH_TOKEN_INVALID` for expired / malformed / wrong-type token
///   or a user that no longer exists
/// - 500 on JWT signing / DB failure
#[tracing::instrument(skip(state, payload))]
pub(crate) async fn refresh(
    State(state): State<AppState>,
    AppJson(payload): AppJson<RefreshRequest>,
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

/// `POST /v1/auth/logout` — auth-gated no-op returning 204.
///
/// Present so clients can drop local state with a single call; refresh-token
/// revocation is tracked in the backlog.
///
/// # Errors
/// - 401 if the access token is missing / invalid
#[tracing::instrument(skip(_current_user, _payload))]
pub(crate) async fn logout(
    _current_user: CurrentUser,
    Json(_payload): Json<LogoutRequest>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

/// `GET /v1/me` — return the authenticated user's profile.
///
/// # Errors
/// - 401 if the access token is missing / invalid
/// - 404 `USER_NOT_FOUND` if the user was deleted between requests
#[tracing::instrument(skip(state), fields(user_id = %current_user.id))]
pub(crate) async fn me(
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
