//! In-memory rate limiter for tests.

use std::collections::HashMap;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{RateLimitPolicy, RateLimitResult, RateLimiter};

struct WindowState {
    count: u64,
    started_at: Instant,
    window_secs: u64,
}

/// Test-only in-memory fixed-window rate limiter.
pub struct InMemoryRateLimiter {
    windows: Mutex<HashMap<String, WindowState>>,
}

impl InMemoryRateLimiter {
    /// Creates an empty rate limiter.
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RateLimiter for InMemoryRateLimiter {
    async fn check(&self, policy: &RateLimitPolicy, identifier: &str) -> Option<RateLimitResult> {
        let key = format!("{}:{}", policy.key_prefix, identifier);
        let mut windows = self.windows.lock().await;

        let now = Instant::now();
        let state = windows.entry(key).or_insert_with(|| WindowState {
            count: 0,
            started_at: now,
            window_secs: policy.window_secs,
        });

        // Reset window if expired
        if now.duration_since(state.started_at).as_secs() >= state.window_secs {
            state.count = 0;
            state.started_at = now;
            state.window_secs = policy.window_secs;
        }

        state.count += 1;
        let elapsed = now.duration_since(state.started_at).as_secs();
        let reset_secs = policy.window_secs.saturating_sub(elapsed);

        let allowed = state.count <= policy.limit;
        let remaining = if allowed {
            policy.limit - state.count
        } else {
            0
        };

        Some(RateLimitResult {
            allowed,
            count: state.count,
            limit: policy.limit,
            remaining,
            reset_secs,
            policy_name: policy.name.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_within_limit() {
        let limiter = InMemoryRateLimiter::new();
        let policy = RateLimitPolicy {
            name: "test".into(),
            limit: 3,
            window_secs: 60,
            key_prefix: "ratelimit:test".into(),
        };

        for i in 1..=3 {
            let result = limiter.check(&policy, "user1").await.unwrap();
            assert!(result.allowed, "request {i} should be allowed");
            assert_eq!(result.remaining, 3 - i);
        }

        let result = limiter.check(&policy, "user1").await.unwrap();
        assert!(!result.allowed, "4th request should be denied");
        assert_eq!(result.remaining, 0);
    }

    #[tokio::test]
    async fn separate_keys_are_independent() {
        let limiter = InMemoryRateLimiter::new();
        let policy = RateLimitPolicy {
            name: "test".into(),
            limit: 1,
            window_secs: 60,
            key_prefix: "ratelimit:test".into(),
        };

        let r1 = limiter.check(&policy, "user1").await.unwrap();
        assert!(r1.allowed);

        let r2 = limiter.check(&policy, "user2").await.unwrap();
        assert!(r2.allowed);
    }

    #[tokio::test]
    async fn result_fields_are_correct() {
        let limiter = InMemoryRateLimiter::new();
        let policy = RateLimitPolicy {
            name: "login".into(),
            limit: 5,
            window_secs: 900,
            key_prefix: "ratelimit:login".into(),
        };

        let r = limiter.check(&policy, "1.2.3.4").await.unwrap();
        assert!(r.allowed);
        assert_eq!(r.count, 1);
        assert_eq!(r.limit, 5);
        assert_eq!(r.remaining, 4);
        assert_eq!(r.policy_name, "login");
    }
}
