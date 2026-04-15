//! JWT signing and verification for user access / refresh tokens.

use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Errors produced while signing or verifying a JWT.
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    /// Token signature parsed but the `exp` claim is in the past.
    #[error("token expired")]
    Expired,
    /// Token is malformed, has a bad signature, or fails validation for any
    /// reason other than expiry.
    #[error("token invalid")]
    Invalid,
}

/// Claims embedded in access and refresh tokens.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// User ID this token was issued to.
    pub sub: Uuid,
    /// Token type: `"access"` or `"refresh"`.
    pub typ: String,
    /// Issued-at time as a unix timestamp (seconds).
    pub iat: i64,
    /// Expiration time as a unix timestamp (seconds).
    pub exp: i64,
}

const ACCESS_TOKEN_HOURS: i64 = 24;
const REFRESH_TOKEN_DAYS: i64 = 30;

/// Signs a 24-hour access token for `user_id`.
///
/// # Errors
/// Returns [`JwtError::Invalid`] if encoding fails.
pub fn sign_access_token(user_id: Uuid, secret: &str) -> Result<String, JwtError> {
    sign_token(
        user_id,
        "access",
        Duration::hours(ACCESS_TOKEN_HOURS),
        secret,
    )
}

/// Signs a 30-day refresh token for `user_id`.
///
/// # Errors
/// Returns [`JwtError::Invalid`] if encoding fails.
pub fn sign_refresh_token(user_id: Uuid, secret: &str) -> Result<String, JwtError> {
    sign_token(
        user_id,
        "refresh",
        Duration::days(REFRESH_TOKEN_DAYS),
        secret,
    )
}

fn sign_token(
    user_id: Uuid,
    typ: &str,
    duration: Duration,
    secret: &str,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let claims = Claims {
        sub: user_id,
        typ: typ.to_string(),
        iat: now.timestamp(),
        exp: (now + duration).timestamp(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| JwtError::Invalid)
}

/// Verifies `token` against `secret` and returns its claims.
///
/// # Errors
/// - [`JwtError::Expired`] if the token's `exp` is in the past.
/// - [`JwtError::Invalid`] for any other validation or parse failure.
pub fn verify_token(token: &str, secret: &str) -> Result<Claims, JwtError> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| {
        use jsonwebtoken::errors::ErrorKind;
        match e.kind() {
            ErrorKind::ExpiredSignature => JwtError::Expired,
            _ => JwtError::Invalid,
        }
    })?;
    Ok(data.claims)
}

/// Seconds until a newly issued access token expires (24 hours).
pub fn access_token_expires_in() -> i64 {
    ACCESS_TOKEN_HOURS * 3600
}
