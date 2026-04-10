use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use common::{AppError, AppState};
use entity::user;
use uuid::Uuid;

/// Authenticated user extracted from Bearer token.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: Uuid,
    pub email: String,
    pub role: user::UserRole,
}

fn extract_bearer_token(parts: &Parts) -> Result<String, AppError> {
    let header = parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("TOKEN_INVALID".to_string()))?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| AppError::Unauthorized("TOKEN_INVALID".to_string()))?;

    Ok(token.to_string())
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_token(parts)?;

        let claims =
            auth::jwt::verify_token(&token, &state.config.jwt_secret).map_err(|e| match e {
                auth::jwt::JwtError::Expired => AppError::Unauthorized("TOKEN_EXPIRED".to_string()),
                auth::jwt::JwtError::Invalid => AppError::Unauthorized("TOKEN_INVALID".to_string()),
            })?;

        if claims.typ != "access" {
            return Err(AppError::Unauthorized("TOKEN_INVALID".to_string()));
        }

        let user_model = state
            .uow
            .user_repo()
            .find_by_id(claims.sub)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .ok_or_else(|| AppError::Unauthorized("TOKEN_INVALID".to_string()))?;

        Ok(CurrentUser {
            id: user_model.id,
            email: user_model.email,
            role: user_model.role,
        })
    }
}
