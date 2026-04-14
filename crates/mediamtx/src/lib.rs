pub mod keys;

use cache::CacheStore;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// A single MediaMTX instance with its internal and public URLs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MtxInstance {
    pub name: String,
    /// Internal API URL for health checks and management (e.g. http://mtx-1:9997)
    pub internal_api: String,
    /// Public WHIP URL for browser push (e.g. http://localhost:8889)
    pub public_whip: String,
    /// Public WHEP URL for browser playback (e.g. http://localhost:8889)
    pub public_whep: String,
    /// Public HLS URL for browser playback (e.g. http://localhost:8888)
    pub public_hls: String,
}

/// Parse MEDIAMTX_INSTANCES JSON into a Vec<MtxInstance>.
/// Returns an empty Vec if the input is empty.
pub fn parse_mtx_instances(json: &str) -> Vec<MtxInstance> {
    if json.is_empty() {
        return Vec::new();
    }
    serde_json::from_str(json)
        .expect("MEDIAMTX_INSTANCES must be a valid JSON array of MtxInstance objects")
}

/// Select the healthiest MediaMTX instance with the lowest stream count.
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

/// Create a new session: generate session_id, write session:* keys, and
/// overwrite stream:{id}:active_session.
///
/// Returns the fresh session_id. The previous active session (if any) is NOT
/// cleaned up here — it will be detected as stale when its webhook arrives.
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

/// Read the currently active session for a stream.
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

/// Read the MTX name associated with a session.
pub async fn get_session_mtx(
    cache: &dyn CacheStore,
    session_id: &Uuid,
) -> Result<Option<String>, anyhow::Error> {
    cache.get(&keys::session_mtx(session_id)).await
}

/// End the currently active session for a stream.
///
/// This is called when an active-session unpublish arrives. It deletes all
/// session:* keys and the stream:{id}:active_session pointer, then returns the
/// mtx_name so the caller can DECR that instance's count.
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

    // Only clear active_session if it still points to us (avoid racing with a
    // newer session that already overwrote it).
    if let Some(sid_str) = stream_id_str {
        if let Ok(stream_id) = sid_str.parse::<Uuid>() {
            let current = cache.get(&keys::stream_active_session(&stream_id)).await?;
            if current.as_deref() == Some(&session_id.to_string()) {
                cache.del(&keys::stream_active_session(&stream_id)).await?;
            }
        }
    }

    if let Some(ref name) = mtx_name {
        // DECR the mtx stream_count.
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

/// Clean up a stale session: deletes session:* keys (but not
/// stream:{id}:active_session, which belongs to a newer session) and DECRs the
/// session's original MTX count.
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

/// Find all streams whose active session currently lives on `mtx_name`.
///
/// Iterates over the DB's live streams (bounded, small set) and checks
/// stream:{id}:active_session → session:{sid}:mtx for each. Returns
/// `(stream_id, session_id)` pairs for callers that need to clean up or
/// migrate those streams (drain / health-check failure).
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

/// Find an MtxInstance by name.
pub fn find_instance<'a>(instances: &'a [MtxInstance], name: &str) -> Option<&'a MtxInstance> {
    instances.iter().find(|i| i.name == name)
}

/// Build stream URLs based on the stream's currently active session.
/// Returns (whep_url, hls_url) or None if there is no active session.
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

/// Health check context for tracking consecutive failures per instance.
pub struct HealthChecker {
    pub cache: Arc<dyn CacheStore>,
    pub instances: Vec<MtxInstance>,
    pub http_client: reqwest::Client,
    /// Track consecutive failure counts per instance name.
    pub failure_counts: std::collections::HashMap<String, u32>,
}

impl HealthChecker {
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

    /// Run one round of health checks for all instances.
    /// Returns list of instances that just crossed the failure threshold (3).
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
