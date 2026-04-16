//! Authentication extractors for bo-api handlers.

use auth::access_state::AccessState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use entity::user;
use entity::user::UserRole;
use error::AppError;
use uuid::Uuid;

use crate::state::BoAppState;

/// Authenticated user resolved from a Bearer JWT access token.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CurrentUser {
    /// User UUID.
    pub id: Uuid,
    /// User email (lowercase).
    pub email: String,
    /// Role controlling authorization.
    pub role: user::UserRole,
}

/// Extracts and validates an admin user from the JWT.
/// Returns 403 if the caller is not an admin or is suspended.
pub struct AdminUser(pub CurrentUser);

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

impl FromRequestParts<BoAppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &BoAppState,
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

        // Access-state check (Redis cache + DB fallback)
        let access = auth::access_state::load_user_access_state(
            state.cache.as_ref(),
            state.uow.db(),
            claims.sub,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        if access == AccessState::Suspended {
            return Err(AppError::Forbidden("ACCOUNT_SUSPENDED".to_string()));
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

impl FromRequestParts<BoAppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &BoAppState,
    ) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if user.role != UserRole::Admin {
            return Err(AppError::Forbidden("admin access required".into()));
        }
        Ok(AdminUser(user))
    }
}
