//! MediaMTX routing, session lifecycle, and health checking.
//!
//! The API server does not proxy media itself; clients connect directly to one
//! of several MediaMTX instances. This crate owns the logic for picking an
//! instance, tracking which stream lives on which instance via Redis, and
//! detecting instance failures.
//!
//! Key Redis layout (see [`keys`]):
//!
//! - `stream:{stream_id}:active_session` → session UUID
//! - `session:{session_id}:mtx` → MTX instance name
//! - `session:{session_id}:stream_id` → stream UUID
//! - `mtx:{name}:stream_count`, `mtx:{name}:status`
#![warn(missing_docs)]

pub mod keys;

use cache::CacheStore;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// A single MediaMTX instance with its internal and public URLs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MtxInstance {
    /// Stable identifier used as the Redis key suffix and in logs.
    pub name: String,
    /// Internal API URL for health checks and management
    /// (e.g. `http://mtx-1:9997`).
    pub internal_api: String,
    /// Public WHIP URL for browser push (e.g. `http://localhost:8889`).
    pub public_whip: String,
    /// Public WHEP URL for browser playback (e.g. `http://localhost:8889`).
    pub public_whep: String,
    /// Public HLS URL for browser playback (e.g. `http://localhost:8888`).
    pub public_hls: String,
}

/// Parses the `MEDIAMTX_INSTANCES` JSON string into a vector of instances.
///
/// An empty input yields an empty vector (for the single-MTX dev setup where
/// the variable is unset).
///
/// # Panics
/// Panics if `json` is non-empty but not a valid JSON array of
/// [`MtxInstance`]. Parsing happens at startup so a loud failure is preferred
/// over silently running with no instances.
pub fn parse_mtx_instances(json: &str) -> Vec<MtxInstance> {
    if json.is_empty() {
        return Vec::new();
    }
    serde_json::from_str(json)
        .expect("MEDIAMTX_INSTANCES must be a valid JSON array of MtxInstance objects")
}

/// Selects the healthy MediaMTX instance with the lowest current stream count.
///
/// Unhealthy instances (per `mtx:{name}:status`) are skipped. Ties are broken
/// by iteration order.
///
/// # Errors
/// Returns an error if no instance is healthy, or if Redis access fails.
#[tracing::instrument(skip(cache, instances))]
pub async fn select_instance(
    cache: &dyn CacheStore,
    instances: &[MtxInstance],
) -> Result<MtxInstance, anyhow::Error> {
    let mut best: Option<(MtxInstance, i64)> = None;

    for inst in instances {
        let status = cache.get(&keys::mtx_status(&inst.name)).await?;
        if status.as_deref() != Some("healthy") {
            tracing::debug!(name = %inst.name, status = ?status, "Instance not healthy, skipping");
            continue;
        }

        let count_key = keys::mtx_stream_count(&inst.name);
        let count: i64 = cache
            .get(&count_key)
            .await?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        match &best {
            Some((_, best_count)) if count >= *best_count => {}
            _ => {
                best = Some((inst.clone(), count));
            }
        }
    }

    best.map(|(inst, count)| {
        tracing::info!(name = %inst.name, stream_count = count, "Selected MTX instance");
        inst
    })
    .ok_or_else(|| anyhow::anyhow!("No healthy MediaMTX instances available"))
}

/// Creates a new session: generates a `session_id`, writes the `session:*`
/// keys, and overwrites `stream:{id}:active_session`.
///
/// The previous active session (if any) is not cleaned up here; its eventual
/// unpublish webhook is detected as stale and cleaned via
/// [`cleanup_stale_session`].
///
/// # Errors
/// Returns an error if any Redis write fails.
#[tracing::instrument(skip(cache))]
pub async fn create_session(
    cache: &dyn CacheStore,
    stream_id: &Uuid,
    mtx_name: &str,
) -> Result<Uuid, anyhow::Error> {
    let session_id = Uuid::new_v4();
    let sid_str = session_id.to_string();

    cache
        .set(&keys::session_mtx(&session_id), mtx_name, None)
        .await?;
    cache
        .set(
            &keys::session_stream_id(&session_id),
            &stream_id.to_string(),
            None,
        )
        .await?;
    cache
        .set(
            &keys::session_started_at(&session_id),
            &Utc::now().to_rfc3339(),
            None,
        )
        .await?;
    cache
        .set(&keys::stream_active_session(stream_id), &sid_str, None)
        .await?;

    tracing::info!(
        %stream_id,
        %session_id,
        mtx_name,
        "Created stream session"
    );
    Ok(session_id)
}

/// Reads the currently active session for `stream_id`, or `None` if the
/// stream has no active session or the stored value is malformed.
pub async fn get_active_session(
    cache: &dyn CacheStore,
    stream_id: &Uuid,
) -> Result<Option<Uuid>, anyhow::Error> {
    let value = cache.get(&keys::stream_active_session(stream_id)).await?;
    match value {
        Some(s) => Ok(s.parse().ok()),
        None => Ok(None),
    }
}

/// Returns the MTX instance name a session was created on, if still recorded.
pub async fn get_session_mtx(
    cache: &dyn CacheStore,
    session_id: &Uuid,
) -> Result<Option<String>, anyhow::Error> {
    cache.get(&keys::session_mtx(session_id)).await
}

/// Ends the currently active session for a stream when its unpublish webhook
/// arrives.
///
/// Deletes all `session:*` keys, clears `stream:{id}:active_session` *only if
/// it still points at this session* (so a newer session that already took
/// over is preserved), and decrements the hosting instance's stream count.
///
/// Returns the MTX instance name the session was hosted on, if known.
///
/// # Errors
/// Returns an error if any Redis operation fails.
#[tracing::instrument(skip(cache))]
pub async fn end_session(
    cache: &dyn CacheStore,
    session_id: &Uuid,
) -> Result<Option<String>, anyhow::Error> {
    let mtx_name = cache.get(&keys::session_mtx(session_id)).await?;
    let stream_id_str = cache.get(&keys::session_stream_id(session_id)).await?;

    cache.del(&keys::session_mtx(session_id)).await?;
    cache.del(&keys::session_stream_id(session_id)).await?;
    cache.del(&keys::session_started_at(session_id)).await?;

    if let Some(sid_str) = stream_id_str {
        if let Ok(stream_id) = sid_str.parse::<Uuid>() {
            let current = cache.get(&keys::stream_active_session(&stream_id)).await?;
            if current.as_deref() == Some(&session_id.to_string()) {
                cache.del(&keys::stream_active_session(&stream_id)).await?;
            }
        }
    }

    if let Some(ref name) = mtx_name {
        let count_key = keys::mtx_stream_count(name);
        let current: i64 = cache
            .get(&count_key)
            .await?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let new_count = (current - 1).max(0);
        cache.set(&count_key, &new_count.to_string(), None).await?;
        tracing::info!(%session_id, mtx_name = %name, new_count, "Ended active session");
    }

    Ok(mtx_name)
}

/// Cleans up a stale session whose unpublish arrived after the stream already
/// moved to a newer session.
///
/// Deletes the stale `session:*` keys (but not `stream:{id}:active_session`,
/// which belongs to the newer session) and decrements the stale session's
/// original MTX count. Returns the MTX instance name.
///
/// # Errors
/// Returns an error if any Redis operation fails.
#[tracing::instrument(skip(cache))]
pub async fn cleanup_stale_session(
    cache: &dyn CacheStore,
    session_id: &Uuid,
) -> Result<Option<String>, anyhow::Error> {
    let mtx_name = cache.get(&keys::session_mtx(session_id)).await?;

    cache.del(&keys::session_mtx(session_id)).await?;
    cache.del(&keys::session_stream_id(session_id)).await?;
    cache.del(&keys::session_started_at(session_id)).await?;

    if let Some(ref name) = mtx_name {
        let count_key = keys::mtx_stream_count(name);
        let current: i64 = cache
            .get(&count_key)
            .await?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let new_count = (current - 1).max(0);
        cache.set(&count_key, &new_count.to_string(), None).await?;
        tracing::info!(%session_id, mtx_name = %name, new_count, "Cleaned up stale session");
    }

    Ok(mtx_name)
}

/// Finds every stream whose active session currently lives on `mtx_name`.
///
/// Used to enumerate streams that must be drained or migrated when an
/// instance is marked unhealthy. Iterates `live_stream_ids` (already
/// bounded by a DB query) and checks each stream's active session →
/// session MTX mapping.
///
/// Returns `(stream_id, session_id)` pairs for the matches.
///
/// # Errors
/// Returns an error if any Redis operation fails.
#[tracing::instrument(skip(cache, live_stream_ids))]
pub async fn get_streams_on_mtx(
    cache: &dyn CacheStore,
    live_stream_ids: &[Uuid],
    mtx_name: &str,
) -> Result<Vec<(Uuid, Uuid)>, anyhow::Error> {
    let mut hits = Vec::new();
    for stream_id in live_stream_ids {
        let Some(session_id) = get_active_session(cache, stream_id).await? else {
            continue;
        };
        let Some(session_mtx) = get_session_mtx(cache, &session_id).await? else {
            continue;
        };
        if session_mtx == mtx_name {
            hits.push((*stream_id, session_id));
        }
    }
    Ok(hits)
}

/// Finds an [`MtxInstance`] by name, or returns `None` if no registered
/// instance matches.
pub fn find_instance<'a>(instances: &'a [MtxInstance], name: &str) -> Option<&'a MtxInstance> {
    instances.iter().find(|i| i.name == name)
}

/// Builds `(whep_url, hls_url)` for a stream based on its active session's
/// current MTX instance.
///
/// Returns `None` if the stream has no active session or the session points
/// to an instance that is no longer in `instances`.
///
/// # Errors
/// Returns an error if any Redis operation fails.
pub async fn resolve_stream_urls(
    cache: &dyn CacheStore,
    instances: &[MtxInstance],
    stream_id: &Uuid,
    stream_key: &str,
) -> Result<Option<(String, String)>, anyhow::Error> {
    let session_id = match get_active_session(cache, stream_id).await? {
        Some(sid) => sid,
        None => return Ok(None),
    };
    let mtx_name = match get_session_mtx(cache, &session_id).await? {
        Some(name) => name,
        None => return Ok(None),
    };

    let instance = match find_instance(instances, &mtx_name) {
        Some(inst) => inst,
        None => {
            tracing::warn!(%stream_id, mtx_name, "Stream mapped to unknown MTX instance");
            return Ok(None);
        }
    };

    let whep_url = format!("{}/{}/whep", instance.public_whep, stream_key);
    let hls_url = format!("{}/{}/index.m3u8", instance.public_hls, stream_key);
    Ok(Some((whep_url, hls_url)))
}

/// Health-check loop state for the registered MTX instances.
///
/// Holds a shared cache handle, the HTTP client used to probe each instance's
/// internal API, and per-instance consecutive failure counts used to decide
/// when to mark an instance unhealthy.
pub struct HealthChecker {
    /// Shared cache for writing `mtx:{name}:status`.
    pub cache: Arc<dyn CacheStore>,
    /// Registered instances to probe.
    pub instances: Vec<MtxInstance>,
    /// HTTP client with a 5s timeout shared across probes.
    pub http_client: reqwest::Client,
    /// Consecutive failure counter keyed by instance name. Reset on success.
    pub failure_counts: std::collections::HashMap<String, u32>,
}

impl HealthChecker {
    /// Creates a checker for `instances` using `cache` to publish status.
    pub fn new(cache: Arc<dyn CacheStore>, instances: Vec<MtxInstance>) -> Self {
        Self {
            cache,
            instances,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            failure_counts: std::collections::HashMap::new(),
        }
    }

    /// Runs one round of health checks and returns the list of instances that
    /// just crossed the failure threshold of 3 consecutive failures.
    ///
    /// Healthy instances get `mtx:{name}:status = "healthy"` with a 30s TTL
    /// (so a crashed instance auto-expires). Unhealthy instances past the
    /// threshold get `"unhealthy"` with no TTL until the next successful
    /// probe.
    pub async fn check_all(&mut self) -> Vec<MtxInstance> {
        let mut newly_failed = Vec::new();

        for inst in &self.instances {
            let url = format!("{}/v3/paths/list", inst.internal_api);
            let healthy = match self.http_client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => true,
                Ok(resp) => {
                    tracing::warn!(name = %inst.name, status = %resp.status(), "MTX health check non-success");
                    false
                }
                Err(e) => {
                    tracing::warn!(name = %inst.name, error = %e, "MTX health check failed");
                    false
                }
            };

            if healthy {
                self.failure_counts.remove(&inst.name);
                if let Err(e) = self
                    .cache
                    .set(&keys::mtx_status(&inst.name), "healthy", Some(30))
                    .await
                {
                    tracing::error!(name = %inst.name, error = %e, "Failed to set health status");
                }
            } else {
                let count = self.failure_counts.entry(inst.name.clone()).or_insert(0);
                *count += 1;
                tracing::warn!(name = %inst.name, consecutive_failures = *count, "MTX unhealthy");

                if *count == 3 {
                    tracing::error!(name = %inst.name, "MTX reached failure threshold (3), triggering cleanup");
                    if let Err(e) = self
                        .cache
                        .set(&keys::mtx_status(&inst.name), "unhealthy", None)
                        .await
                    {
                        tracing::error!(name = %inst.name, error = %e, "Failed to set unhealthy status");
                    }
                    newly_failed.push(inst.clone());
                }
            }
        }

        newly_failed
    }
}
