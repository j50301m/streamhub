//! streamhub HTTP API server.
//!
//! Axum-based REST + WebSocket service that orchestrates the streaming
//! lifecycle: authenticates users, issues stream tokens, routes publishers to
//! MediaMTX instances, receives MediaMTX webhooks, drives VOD transcoding, and
//! pushes live updates (live stream list, viewer counts) to browsers over WS.
//!
//! Media itself never flows through this crate — WHIP / WHEP / HLS is handled
//! by MediaMTX; the API is the business-logic and routing brain.
#![warn(missing_docs)]

use anyhow::Result;

/// Custom Axum extractors (e.g. unified JSON extractor producing `AppError`).
pub mod extractors;
/// HTTP route handlers grouped by resource.
pub mod handlers;
mod initializer;
mod log_format;
/// Axum middleware (auth, metrics).
pub mod middleware;
mod routes;
mod tasks;
/// WebSocket connection manager and message types.
pub mod ws;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<()> {
    let app = initializer::App::init().await?;
    app.run().await
}
