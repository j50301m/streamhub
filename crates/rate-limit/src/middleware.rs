//! Axum/Tower rate-limit middleware layer.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use metrics::counter;
use tower::{Layer, Service};

use crate::{ClientIp, RateLimitPolicy, RateLimitResult, RateLimiter};

/// How the middleware resolves the rate-limit key.
#[derive(Debug, Clone)]
pub enum RateLimitMode {
    /// Use IP address (from X-Forwarded-For / ConnectInfo).
    Ip,
    /// Use authenticated user_id extracted from `x-ratelimit-user-id` extension
    /// header (injected by auth middleware / extractor). Falls back to IP if not
    /// present (i.e. unauthenticated request).
    UserId,
    /// Use authenticated user_id; skip if not present (no-op for unauthed).
    UserIdOnly,
}

/// Tower layer that wraps services with rate limiting.
#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<dyn RateLimiter>,
    policy: RateLimitPolicy,
    mode: RateLimitMode,
}

impl RateLimitLayer {
    /// Create a new rate-limit layer.
    pub fn new(
        limiter: Arc<dyn RateLimiter>,
        policy: RateLimitPolicy,
        mode: RateLimitMode,
    ) -> Self {
        Self {
            limiter,
            policy,
            mode,
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
            policy: self.policy.clone(),
            mode: self.mode.clone(),
        }
    }
}

/// The middleware service produced by [`RateLimitLayer`].
#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<dyn RateLimiter>,
    policy: RateLimitPolicy,
    mode: RateLimitMode,
}

impl<S> Service<Request<Body>> for RateLimitService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let limiter = self.limiter.clone();
        let policy = self.policy.clone();
        let mode = self.mode.clone();
        let mut inner = self.inner.clone();
        // Swap so we have the ready instance
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            let identifier = resolve_identifier(&req, &mode);

            // Skip if no identifier can be resolved (UserIdOnly with no user)
            let Some(identifier) = identifier else {
                return inner.call(req).await;
            };

            match limiter.check(&policy, &identifier).await {
                Some(result) => {
                    record_metric(&policy.name, &result);

                    if !result.allowed {
                        return Ok(crate::make_rate_limited_response(&result));
                    }

                    let mut response = inner.call(req).await?;
                    crate::inject_rate_limit_headers(response.headers_mut(), &result);
                    Ok(response)
                }
                None => {
                    // Fail-open: Redis unavailable, no headers
                    inner.call(req).await
                }
            }
        })
    }
}

fn resolve_identifier(req: &Request<Body>, mode: &RateLimitMode) -> Option<String> {
    match mode {
        RateLimitMode::Ip => {
            let ip = ClientIp::from_parts(&parts_from_request(req));
            Some(ip.0)
        }
        RateLimitMode::UserId => {
            // Try user_id extension first, fallback to IP
            if let Some(user_id) = req.extensions().get::<RateLimitUserId>() {
                Some(user_id.0.clone())
            } else {
                let ip = ClientIp::from_parts(&parts_from_request(req));
                Some(ip.0)
            }
        }
        RateLimitMode::UserIdOnly => req
            .extensions()
            .get::<RateLimitUserId>()
            .map(|u| u.0.clone()),
    }
}

/// Extension type inserted into the request by auth middleware so the
/// rate-limit middleware can use user_id as the key.
#[derive(Debug, Clone)]
pub struct RateLimitUserId(pub String);

fn record_metric(policy_name: &str, result: &RateLimitResult) {
    let outcome = if result.allowed {
        "allowed"
    } else {
        "rejected"
    };
    counter!("rate_limit_hits_total", "endpoint" => policy_name.to_string(), "result" => outcome)
        .increment(1);
}

/// Extract parts from a request without consuming it.
fn parts_from_request(req: &Request<Body>) -> axum::http::request::Parts {
    let mut builder = axum::http::Request::builder();
    for (name, value) in req.headers() {
        builder = builder.header(name, value);
    }
    let dummy = builder.body(()).unwrap();
    let (mut parts, _) = dummy.into_parts();
    parts.extensions = req.extensions().clone();
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::{InMemoryRateLimiter, RateLimitPolicy};

    fn test_policy(name: &str, limit: u64, window_secs: u64) -> RateLimitPolicy {
        RateLimitPolicy {
            name: name.into(),
            limit,
            window_secs,
            key_prefix: format!("ratelimit:{name}"),
        }
    }

    async fn ok_handler() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn allows_within_limit() {
        let limiter = Arc::new(InMemoryRateLimiter::new());
        let policy = test_policy("test", 3, 60);

        let app = axum::Router::new()
            .route("/test", get(ok_handler))
            .layer(RateLimitLayer::new(limiter, policy, RateLimitMode::Ip));

        for _ in 0..3 {
            let req = Request::builder()
                .uri("/test")
                .header("x-forwarded-for", "1.2.3.4")
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(resp.headers().contains_key("x-ratelimit-limit"));
            assert!(resp.headers().contains_key("x-ratelimit-remaining"));
            assert!(resp.headers().contains_key("x-ratelimit-reset"));
        }
    }

    #[tokio::test]
    async fn returns_429_when_exceeded() {
        let limiter = Arc::new(InMemoryRateLimiter::new());
        let policy = test_policy("test", 2, 60);

        let app = axum::Router::new()
            .route("/test", get(ok_handler))
            .layer(RateLimitLayer::new(limiter, policy, RateLimitMode::Ip));

        // First two should succeed
        for _ in 0..2 {
            let req = Request::builder()
                .uri("/test")
                .header("x-forwarded-for", "1.2.3.4")
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // Third should be rate limited
        let req = Request::builder()
            .uri("/test")
            .header("x-forwarded-for", "1.2.3.4")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().contains_key("retry-after"));

        // Parse body
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "RATE_LIMITED");
        assert!(json["error"]["details"]["retry_after_seconds"].is_number());
    }

    #[tokio::test]
    async fn user_id_only_skips_unauthed() {
        let limiter = Arc::new(InMemoryRateLimiter::new());
        let policy = test_policy("authed", 1, 60);

        let app = axum::Router::new()
            .route("/test", get(ok_handler))
            .layer(RateLimitLayer::new(
                limiter,
                policy,
                RateLimitMode::UserIdOnly,
            ));

        // Without user_id extension, should pass through (no rate limit)
        for _ in 0..5 {
            let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            // No rate limit headers since UserIdOnly skipped
            assert!(!resp.headers().contains_key("x-ratelimit-limit"));
        }
    }

    #[tokio::test]
    async fn user_id_only_limits_authed() {
        let limiter = Arc::new(InMemoryRateLimiter::new());
        let policy = test_policy("authed", 1, 60);

        let app = axum::Router::new()
            .route("/test", get(ok_handler))
            .layer(RateLimitLayer::new(
                limiter,
                policy,
                RateLimitMode::UserIdOnly,
            ));

        // First request with user_id succeeds
        let mut req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(RateLimitUserId("user-123".into()));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second request with same user_id is rate limited
        let mut req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(RateLimitUserId("user-123".into()));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn different_ips_have_separate_limits() {
        let limiter = Arc::new(InMemoryRateLimiter::new());
        let policy = test_policy("test", 1, 60);

        let app = axum::Router::new()
            .route("/test", get(ok_handler))
            .layer(RateLimitLayer::new(limiter, policy, RateLimitMode::Ip));

        let req = Request::builder()
            .uri("/test")
            .header("x-forwarded-for", "1.1.1.1")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let req = Request::builder()
            .uri("/test")
            .header("x-forwarded-for", "2.2.2.2")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn headers_show_remaining_count() {
        let limiter = Arc::new(InMemoryRateLimiter::new());
        let policy = test_policy("test", 5, 60);

        let app = axum::Router::new()
            .route("/test", get(ok_handler))
            .layer(RateLimitLayer::new(limiter, policy, RateLimitMode::Ip));

        let req = Request::builder()
            .uri("/test")
            .header("x-forwarded-for", "1.2.3.4")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("x-ratelimit-limit").unwrap(), "5");
        assert_eq!(resp.headers().get("x-ratelimit-remaining").unwrap(), "4");
    }
}
