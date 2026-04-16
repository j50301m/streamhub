//! Axum HTTP handlers grouped by resource.
//!
//! Each submodule implements the handlers for one route family and defines
//! its request / response DTOs. Routing is wired up in `crate::routes`.

/// Admin console endpoints.
pub mod admin;
/// User auth: register / login / refresh / logout / `/v1/me`.
pub mod auth;
/// Live chat: subscribe/send/history via WebSocket + Redis Streams.
pub mod chat;
/// Chat moderation: delete messages, ban/unban users.
pub mod chat_moderation;
/// MTX drain: mark a MediaMTX instance as draining and evict clients.
pub mod drain;
/// Health probes for DB and Redis.
pub mod health;
/// MediaMTX HTTP auth callback for publish / read actions.
pub mod mediamtx_auth;
/// Prometheus `/metrics` exposition endpoint.
pub mod metrics;
/// MediaMTX publish / unpublish webhook.
pub mod publish;
/// MediaMTX recording-segment-complete webhook.
pub mod recording;
/// Stream CRUD + token issuance + live/VOD listings.
pub mod streams;
/// Stream thumbnail upload.
pub mod thumbnail;
/// GCP Transcoder job state-change Pub/Sub push receiver.
pub mod transcoder_webhook;
/// WebSocket upgrade endpoint for real-time events.
pub mod ws;
