use std::sync::Arc;

use crate::config::AppConfig;
use crate::state::AppState;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use rate_limit::{RateLimitLayer, RateLimitMode, RateLimitPolicy, RateLimiter};
use serde_json::{Value, json};

use crate::handlers;

async fn healthz() -> axum::Json<Value> {
    axum::Json(json!({"status": "ok"}))
}

pub fn app_router(limiter: Arc<dyn RateLimiter>, config: &AppConfig) -> Router<AppState> {
    let register_policy = RateLimitPolicy {
        name: "register".into(),
        limit: config.rate_limit_register_limit,
        window_secs: config.rate_limit_register_window,
        key_prefix: "ratelimit:register".into(),
    };
    let login_policy = RateLimitPolicy {
        name: "login".into(),
        limit: config.rate_limit_login_limit,
        window_secs: config.rate_limit_login_window,
        key_prefix: "ratelimit:login".into(),
    };
    let stream_token_policy = RateLimitPolicy {
        name: "stream_token".into(),
        limit: config.rate_limit_stream_token_limit,
        window_secs: config.rate_limit_stream_token_window,
        key_prefix: "ratelimit:stream_token".into(),
    };
    let ws_policy = RateLimitPolicy {
        name: "ws".into(),
        limit: config.rate_limit_ws_limit,
        window_secs: config.rate_limit_ws_window,
        key_prefix: "ratelimit:ws".into(),
    };

    Router::new()
        // Health & Metrics
        .route("/healthz", get(healthz))
        .route("/v1/health", get(handlers::health::health_check))
        .route("/metrics", get(handlers::metrics::metrics_handler))
        // Auth — with endpoint-specific rate limits
        .route(
            "/v1/auth/register",
            post(handlers::auth::register).layer(RateLimitLayer::new(
                limiter.clone(),
                register_policy,
                RateLimitMode::Ip,
            )),
        )
        .route(
            "/v1/auth/login",
            post(handlers::auth::login).layer(RateLimitLayer::new(
                limiter.clone(),
                login_policy,
                RateLimitMode::Ip,
            )),
        )
        .route("/v1/auth/refresh", post(handlers::auth::refresh))
        .route("/v1/auth/logout", post(handlers::auth::logout))
        .route("/v1/me", get(handlers::auth::me))
        // Streams
        .route(
            "/v1/streams",
            post(handlers::streams::create_stream).get(handlers::streams::list_streams),
        )
        .route(
            "/v1/streams/live",
            get(handlers::streams::list_live_streams),
        )
        .route("/v1/streams/vod", get(handlers::streams::list_vod_streams))
        .route(
            "/v1/streams/{id}",
            get(handlers::streams::get_stream)
                .patch(handlers::streams::update_stream)
                .delete(handlers::streams::delete_stream),
        )
        .route("/v1/streams/{id}/end", post(handlers::streams::end_stream))
        .route(
            "/v1/streams/{id}/token",
            post(handlers::streams::create_stream_token).layer(RateLimitLayer::new(
                limiter.clone(),
                stream_token_policy,
                RateLimitMode::UserIdOnly,
            )),
        )
        .route(
            "/v1/streams/{id}/thumbnail",
            post(handlers::thumbnail::upload_thumbnail)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route(
            "/v1/streams/{id}/recordings",
            get(handlers::streams::list_recordings),
        )
        // Chat moderation
        .route(
            "/v1/streams/{id}/chat/messages/{msg_id}",
            delete(handlers::chat_moderation::delete_message_handler),
        )
        .route(
            "/v1/streams/{id}/chat/bans",
            post(handlers::chat_moderation::ban_user_handler)
                .get(handlers::chat_moderation::list_bans_handler),
        )
        .route(
            "/v1/streams/{id}/chat/bans/{user_id}",
            delete(handlers::chat_moderation::unban_user_handler),
        )
        // WebSocket — with rate limit
        .route(
            "/v1/ws",
            get(handlers::ws::ws_handler).layer(RateLimitLayer::new(
                limiter.clone(),
                ws_policy,
                RateLimitMode::Ip,
            )),
        )
        // Internal hooks (no rate limit)
        .route(
            "/internal/hooks/publish",
            post(handlers::publish::publish_hook),
        )
        .route(
            "/internal/hooks/recording",
            post(handlers::recording::recording_hook),
        )
        .route(
            "/internal/hooks/transcoder-complete",
            post(handlers::transcoder_webhook::transcoder_webhook),
        )
        .route(
            "/internal/auth",
            post(handlers::mediamtx_auth::mediamtx_auth),
        )
        // MTX drain
        .route("/internal/mtx/drain", post(handlers::drain::drain_handler))
}
