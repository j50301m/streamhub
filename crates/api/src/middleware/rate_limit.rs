//! Global unauthed rate-limit middleware that skips `/internal/*` routes.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use rate_limit::RateLimitUserId;
use rate_limit::{ClientIp, RateLimitPolicy, RateLimiter};

/// Unauthed rate limit middleware. Skips `/internal/*` and `/healthz` paths.
/// Uses IP as the rate limit key.
pub async fn unauthed_rate_limit(
    State((limiter, policy)): State<(Arc<dyn RateLimiter>, RateLimitPolicy)>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();

    // Skip internal routes and health checks
    if path.starts_with("/internal/") || path == "/healthz" || path == "/metrics" {
        return next.run(req).await;
    }

    // Requests that already resolved to an authenticated user should only
    // consume the authed bucket, not the shared unauthed IP bucket.
    if req.extensions().get::<RateLimitUserId>().is_some() {
        return next.run(req).await;
    }

    let ip = extract_ip(&req);

    match limiter.check(&policy, &ip).await {
        Some(result) => {
            metrics::counter!(
                "rate_limit_hits_total",
                "endpoint" => policy.name.clone(),
                "result" => if result.allowed { "allowed" } else { "rejected" }
            )
            .increment(1);

            if !result.allowed {
                return rate_limit::make_rate_limited_response(&result);
            }

            let mut response = next.run(req).await;
            rate_limit::inject_rate_limit_headers(response.headers_mut(), &result);
            response
        }
        None => {
            // Fail-open: Redis unavailable
            next.run(req).await
        }
    }
}

fn extract_ip(req: &Request) -> String {
    // Build minimal Parts for ClientIp extraction
    let mut builder = axum::http::Request::builder();
    for (name, value) in req.headers() {
        builder = builder.header(name, value);
    }
    let dummy = builder.body(()).unwrap();
    let (mut parts, _) = dummy.into_parts();
    parts.extensions = req.extensions().clone();
    ClientIp::from_parts(&parts).0
}
