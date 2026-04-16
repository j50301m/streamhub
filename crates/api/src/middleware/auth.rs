use crate::state::AppState;
use auth::access_state::AccessState;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use entity::user;
use error::AppError;
use rate_limit::RateLimitUserId;
use uuid::Uuid;

/// Authenticated user resolved from a Bearer JWT access token.
///
/// Use as a handler parameter to require authentication; extraction fails with
/// `AppError::Unauthorized` when the token is missing, malformed, expired, or
/// the user no longer exists. Also rejects suspended users with 403.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    /// User UUID.
    pub id: Uuid,
    /// User email (lowercase).
    pub email: String,
    /// Role controlling authorization (broadcaster / viewer / admin).
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

/// Lightweight middleware that extracts user_id from a valid JWT (if present)
/// and inserts it as a [`RateLimitUserId`] extension. Does NOT reject invalid
/// tokens — that is left to the [`CurrentUser`] extractor in handlers.
pub async fn inject_user_id_extension(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(header) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = header.strip_prefix("Bearer ") {
            if let Ok(claims) = auth::jwt::verify_token(token, &state.config.jwt_secret) {
                if claims.typ == "access" {
                    req.extensions_mut()
                        .insert(RateLimitUserId(claims.sub.to_string()));
                }
            }
        }
    }
    next.run(req).await
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
