//! Redis-backed rate limiter using a Lua script for atomic fixed-window counting.

use async_trait::async_trait;
use deadpool_redis::Pool as RedisPool;

use crate::{RateLimitPolicy, RateLimitResult, RateLimiter};

/// Lua script: atomic INCR + EXPIRE fixed-window counter.
///
/// KEYS[1] = rate limit key
/// ARGV[1] = limit (for return value only)
/// ARGV[2] = window in seconds
///
/// Returns {allowed (0|1), current_count, ttl}.
const LUA_SCRIPT: &str = r#"
local key = KEYS[1]
local limit = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local current = redis.call("INCR", key)
if current == 1 then
    redis.call("EXPIRE", key, window)
end
local ttl = redis.call("TTL", key)
if current > limit then
    return {0, current, ttl}
end
return {1, current, ttl}
"#;

/// Rate limiter backed by Redis Lua script (fixed window counter).
pub struct RedisRateLimiter {
    pool: RedisPool,
}

impl RedisRateLimiter {
    /// Creates a new Redis-backed rate limiter.
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RateLimiter for RedisRateLimiter {
    async fn check(&self, policy: &RateLimitPolicy, identifier: &str) -> Option<RateLimitResult> {
        let key = format!("{}:{}", policy.key_prefix, identifier);

        let mut conn = match self.pool.get().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, key = %key, "rate-limit: Redis pool unavailable, fail-open");
                return None;
            }
        };

        let result: Vec<i64> = match deadpool_redis::redis::cmd("EVAL")
            .arg(LUA_SCRIPT)
            .arg(1) // number of keys
            .arg(&key)
            .arg(policy.limit)
            .arg(policy.window_secs)
            .query_async(&mut *conn)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, key = %key, "rate-limit: Lua eval failed, fail-open");
                return None;
            }
        };

        if result.len() < 3 {
            tracing::warn!(key = %key, "rate-limit: unexpected Lua result length, fail-open");
            return None;
        }

        let allowed = result[0] == 1;
        let count = result[1] as u64;
        let ttl = result[2].max(0) as u64;

        let remaining = if allowed {
            policy.limit.saturating_sub(count)
        } else {
            0
        };

        Some(RateLimitResult {
            allowed,
            count,
            limit: policy.limit,
            remaining,
            reset_secs: ttl,
            policy_name: policy.name.clone(),
        })
    }
}
