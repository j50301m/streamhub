//! Rate limiting for streamhub API and bo-api.
//!
//! Provides a [`RateLimiter`] trait with Redis (Lua script fixed window counter)
//! and in-memory (test) implementations, plus Axum middleware and IP extraction.
#![warn(missing_docs)]

mod ip;
mod memory;
mod middleware;
mod redis;

pub use ip::ClientIp;
pub use memory::InMemoryRateLimiter;
pub use middleware::{RateLimitLayer, RateLimitMode, RateLimitUserId};
pub use redis::RedisRateLimiter;

use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// A rate-limiting policy describing the allowed request rate.
#[derive(Debug, Clone)]
pub struct RateLimitPolicy {
    /// Human-readable name used as the Prometheus `endpoint` label.
    pub name: String,
    /// Maximum number of requests allowed in the window.
    pub limit: u64,
    /// Window duration in seconds.
    pub window_secs: u64,
    /// Redis key prefix (e.g. `"ratelimit:login"`). The full key is
    /// `{key_prefix}:{identifier}` where identifier is user_id or IP.
    pub key_prefix: String,
}

/// Result of a rate-limit check.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// Current request count in this window.
    pub count: u64,
    /// Configured limit for this policy.
    pub limit: u64,
    /// Remaining requests before hitting the limit.
    pub remaining: u64,
    /// Seconds until the current window resets.
    pub reset_secs: u64,
    /// Policy name (for metrics / header selection).
    pub policy_name: String,
}

/// Trait abstracting rate-limit backends (Redis, in-memory).
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// Check (and consume) one request against `policy` for `identifier`.
    async fn check(&self, policy: &RateLimitPolicy, identifier: &str) -> Option<RateLimitResult>;
}

/// Build a 429 Too Many Requests response from a rate-limit result.
pub fn make_rate_limited_response(result: &RateLimitResult) -> Response {
    let retry_after = result.reset_secs;
    let body = serde_json::json!({
        "error": {
            "code": "RATE_LIMITED",
            "message": "Too many requests, please try again later",
            "details": {
                "retry_after_seconds": retry_after
            }
        }
    });

    let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
    inject_rate_limit_headers(response.headers_mut(), result);
    if let Ok(val) = HeaderValue::from_str(&retry_after.to_string()) {
        response.headers_mut().insert("Retry-After", val);
    }
    response
}

/// Inject `X-RateLimit-*` headers into a response.
pub fn inject_rate_limit_headers(headers: &mut HeaderMap, result: &RateLimitResult) {
    let reset_epoch = chrono::Utc::now().timestamp() as u64 + result.reset_secs;

    if let Ok(v) = HeaderValue::from_str(&result.limit.to_string()) {
        headers.insert("X-RateLimit-Limit", v);
    }
    if let Ok(v) = HeaderValue::from_str(&result.remaining.to_string()) {
        headers.insert("X-RateLimit-Remaining", v);
    }
    if let Ok(v) = HeaderValue::from_str(&reset_epoch.to_string()) {
        headers.insert("X-RateLimit-Reset", v);
    }
}
