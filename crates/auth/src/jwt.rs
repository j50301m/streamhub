use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("token expired")]
    Expired,
    #[error("token invalid")]
    Invalid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — user ID
    pub sub: Uuid,
    /// Token type: "access" or "refresh"
    pub typ: String,
    /// Issued at (unix timestamp)
    pub iat: i64,
    /// Expiration (unix timestamp)
    pub exp: i64,
}

/// Access token validity: 24 hours.
const ACCESS_TOKEN_HOURS: i64 = 24;
/// Refresh token validity: 30 days.
const REFRESH_TOKEN_DAYS: i64 = 30;

/// Sign an access token (24h).
pub fn sign_access_token(user_id: Uuid, secret: &str) -> Result<String, JwtError> {
    sign_token(
        user_id,
        "access",
        Duration::hours(ACCESS_TOKEN_HOURS),
        secret,
    )
}

/// Sign a refresh token (30d).
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

/// Verify and decode a JWT token. Returns claims if valid.
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

/// Returns the number of seconds until an access token expires (24h).
pub fn access_token_expires_in() -> i64 {
    ACCESS_TOKEN_HOURS * 3600
}
