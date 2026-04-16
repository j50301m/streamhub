use axum::Json;
use axum::extract::State;
use chrono::Utc;
use entity::user;
use error::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::BoAppState;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub role: user::UserRole,
    pub created_at: chrono::DateTime<Utc>,
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

/// `POST /v1/auth/login` — verify credentials and issue tokens.
#[tracing::instrument(skip(state, payload), fields(email = %payload.email))]
pub async fn login(
    State(state): State<BoAppState>,
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

    Ok(Json(DataResponse {
        data: AuthResponse {
            user: user_to_response(&model),
            access_token,
            refresh_token,
            expires_in: auth::jwt::access_token_expires_in(),
        },
    }))
}

/// `POST /v1/auth/refresh` — exchange a valid refresh token for a new pair.
#[tracing::instrument(skip(state, payload))]
pub async fn refresh(
    State(state): State<BoAppState>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Json<DataResponse<TokenResponse>>, AppError> {
    let claims = auth::jwt::verify_token(&payload.refresh_token, &state.config.jwt_secret)
        .map_err(|_| AppError::Unauthorized("REFRESH_TOKEN_INVALID".to_string()))?;

    if claims.typ != "refresh" {
        return Err(AppError::Unauthorized("REFRESH_TOKEN_INVALID".to_string()));
    }

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
