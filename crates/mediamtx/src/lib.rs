use cache::CacheStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
///
/// 1. Filter instances where `mtx:{name}:status` == "healthy"
/// 2. For each healthy instance, read `mtx:{name}:stream_count` (default 0)
/// 3. Return the instance with the lowest count
/// 4. If all unhealthy/draining/missing, return an error
#[tracing::instrument(skip(cache, instances))]
pub async fn select_instance(
    cache: &dyn CacheStore,
    instances: &[MtxInstance],
) -> Result<MtxInstance, anyhow::Error> {
    let mut best: Option<(MtxInstance, i64)> = None;

    for inst in instances {
        let status = cache.get(&format!("mtx:{}:status", inst.name)).await?;
        if status.as_deref() != Some("healthy") {
            tracing::debug!(name = %inst.name, status = ?status, "Instance not healthy, skipping");
            continue;
        }

        let count_key = format!("mtx:{}:stream_count", inst.name);
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

/// Set the stream→MTX mapping in Redis without incrementing the stream count.
/// Used during token creation to reserve the mapping before the stream is actually live.
#[tracing::instrument(skip(cache))]
pub async fn set_stream_mapping(
    cache: &dyn CacheStore,
    stream_id: &str,
    mtx_name: &str,
) -> Result<(), anyhow::Error> {
    cache
        .set(&format!("stream:{stream_id}:mtx"), mtx_name, None)
        .await?;

    tracing::info!(
        stream_id,
        mtx_name,
        "Set stream→MTX mapping (no count increment)"
    );
    Ok(())
}

/// Record a stream→MTX mapping in Redis and increment the instance's stream count.
#[tracing::instrument(skip(cache))]
pub async fn record_stream_mapping(
    cache: &dyn CacheStore,
    stream_id: &str,
    mtx_name: &str,
) -> Result<(), anyhow::Error> {
    // SET stream:{stream_id}:mtx → mtx_name
    cache
        .set(&format!("stream:{stream_id}:mtx"), mtx_name, None)
        .await?;

    // INCR mtx:{name}:stream_count
    // CacheStore doesn't have INCR, so we read-modify-write.
    // This is fine since publish hooks are serialized per stream.
    let count_key = format!("mtx:{mtx_name}:stream_count");
    let current: i64 = cache
        .get(&count_key)
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    cache
        .set(&count_key, &(current + 1).to_string(), None)
        .await?;

    tracing::info!(
        stream_id,
        mtx_name,
        new_count = current + 1,
        "Recorded stream→MTX mapping"
    );
    Ok(())
}

/// Remove a stream→MTX mapping from Redis and decrement the instance's stream count.
#[tracing::instrument(skip(cache))]
pub async fn remove_stream_mapping(
    cache: &dyn CacheStore,
    stream_id: &str,
) -> Result<Option<String>, anyhow::Error> {
    let mapping_key = format!("stream:{stream_id}:mtx");
    let mtx_name = cache.get(&mapping_key).await?;

    if let Some(ref name) = mtx_name {
        cache.del(&mapping_key).await?;

        let count_key = format!("mtx:{name}:stream_count");
        let current: i64 = cache
            .get(&count_key)
            .await?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let new_count = (current - 1).max(0);
        cache.set(&count_key, &new_count.to_string(), None).await?;

        tracing::info!(stream_id, mtx_name = %name, new_count, "Removed stream→MTX mapping");
    }

    Ok(mtx_name)
}

/// Look up which MTX instance a stream is assigned to.
pub async fn get_stream_mtx(
    cache: &dyn CacheStore,
    stream_id: &str,
) -> Result<Option<String>, anyhow::Error> {
    cache.get(&format!("stream:{stream_id}:mtx")).await
}

/// Find an MtxInstance by name.
pub fn find_instance<'a>(instances: &'a [MtxInstance], name: &str) -> Option<&'a MtxInstance> {
    instances.iter().find(|i| i.name == name)
}

/// Build stream URLs based on the assigned MTX instance.
/// Returns (whep_url, hls_url) or None if stream is not mapped.
pub async fn resolve_stream_urls(
    cache: &dyn CacheStore,
    instances: &[MtxInstance],
    stream_id: &str,
    stream_key: &str,
) -> Result<Option<(String, String)>, anyhow::Error> {
    let mtx_name = match get_stream_mtx(cache, stream_id).await? {
        Some(name) => name,
        None => return Ok(None),
    };

    let instance = match find_instance(instances, &mtx_name) {
        Some(inst) => inst,
        None => {
            tracing::warn!(stream_id, mtx_name, "Stream mapped to unknown MTX instance");
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
                // Reset failure count and mark healthy (with TTL so it auto-expires to missing = unhealthy)
                self.failure_counts.remove(&inst.name);
                if let Err(e) = self
                    .cache
                    .set(&format!("mtx:{}:status", inst.name), "healthy", Some(30))
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
                    // Mark as unhealthy explicitly (no TTL)
                    if let Err(e) = self
                        .cache
                        .set(&format!("mtx:{}:status", inst.name), "unhealthy", None)
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
