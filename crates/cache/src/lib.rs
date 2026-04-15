//! Cache and pub/sub abstractions used across the workspace.
//!
//! Two traits are exposed: [`CacheStore`] for key/value storage with optional
//! TTL (session state, MTX routing counters, stream tokens), and [`PubSub`]
//! for cross-instance event fan-out (viewer count updates, stream lifecycle
//! notifications). Each has a Redis-backed production impl and an in-memory
//! impl used by tests.
#![warn(missing_docs)]

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use deadpool_redis::Pool as RedisPool;
use deadpool_redis::redis::AsyncCommands;
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;

/// A single entry returned by [`CacheStore::xrevrange`] or produced by
/// [`CacheStore::xadd_maxlen`]. `id` is the Redis stream entry id (e.g.
/// `"1712345678901-0"`) and `fields` is the ordered list of field/value pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEntry {
    /// Redis Stream entry id (millisecond timestamp + sequence).
    pub id: String,
    /// Field/value pairs attached to this entry.
    pub fields: Vec<(String, String)>,
}

/// Key-value cache with optional per-key TTL. All implementations are `Send + Sync`.
#[async_trait]
pub trait CacheStore: Send + Sync {
    /// Returns the value for `key`, or `None` if it is not set.
    async fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error>;

    /// Stores `value` at `key`. If `ttl_secs` is `Some`, the key expires after
    /// that many seconds; otherwise it lives until explicitly deleted.
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>)
    -> Result<(), anyhow::Error>;

    /// Deletes `key`. Missing keys are not an error.
    async fn del(&self, key: &str) -> Result<(), anyhow::Error>;

    /// SET if Not eXists. Returns `true` if the key was set, `false` if it
    /// already existed. When `ttl_secs` is `Some`, the key expires after the
    /// given seconds. Used to implement distributed locks.
    async fn set_nx(
        &self,
        key: &str,
        value: &str,
        ttl_secs: Option<u64>,
    ) -> Result<bool, anyhow::Error>;

    /// Sets a TTL (in seconds) on an existing key. No-op if the key doesn't
    /// exist. Used for [`xadd_maxlen`](Self::xadd_maxlen) streams that should
    /// expire after a period of inactivity.
    async fn expire(&self, key: &str, ttl_secs: u64) -> Result<(), anyhow::Error>;

    /// XADD to a Redis Stream at `key` with `MAXLEN ~ max_len` approximate
    /// trimming. `fields` are the ordered (name, value) pairs. Returns the
    /// assigned entry id.
    async fn xadd_maxlen(
        &self,
        key: &str,
        max_len: usize,
        fields: &[(&str, &str)],
    ) -> Result<String, anyhow::Error>;

    /// XREVRANGE reading up to `count` entries from newest to oldest between
    /// the open endpoints `"+"` and `"-"`.
    async fn xrevrange(&self, key: &str, count: usize) -> Result<Vec<StreamEntry>, anyhow::Error>;
}

/// In-memory [`CacheStore`] backed by a `HashMap`. TTLs are accepted but not
/// enforced; intended for unit tests only.
pub struct InMemoryCache {
    data: Mutex<HashMap<String, String>>,
    streams: Mutex<HashMap<String, VecDeque<StreamEntry>>>,
    stream_seq: Mutex<u64>,
}

impl InMemoryCache {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            stream_seq: Mutex::new(0),
        }
    }
}

impl Default for InMemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CacheStore for InMemoryCache {
    async fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error> {
        let data = self.data.lock().await;
        Ok(data.get(key).cloned())
    }

    async fn set(
        &self,
        key: &str,
        value: &str,
        _ttl_secs: Option<u64>,
    ) -> Result<(), anyhow::Error> {
        let mut data = self.data.lock().await;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn del(&self, key: &str) -> Result<(), anyhow::Error> {
        let mut data = self.data.lock().await;
        data.remove(key);
        Ok(())
    }

    async fn set_nx(
        &self,
        key: &str,
        value: &str,
        _ttl_secs: Option<u64>,
    ) -> Result<bool, anyhow::Error> {
        let mut data = self.data.lock().await;
        if data.contains_key(key) {
            Ok(false)
        } else {
            data.insert(key.to_string(), value.to_string());
            Ok(true)
        }
    }

    async fn expire(&self, _key: &str, _ttl_secs: u64) -> Result<(), anyhow::Error> {
        Ok(())
    }

    async fn xadd_maxlen(
        &self,
        key: &str,
        max_len: usize,
        fields: &[(&str, &str)],
    ) -> Result<String, anyhow::Error> {
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut seq = self.stream_seq.lock().await;
        *seq += 1;
        let id = format!("{ts_ms}-{seq}");
        drop(seq);

        let entry = StreamEntry {
            id: id.clone(),
            fields: fields
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        };

        let mut streams = self.streams.lock().await;
        let buf = streams.entry(key.to_string()).or_default();
        buf.push_back(entry);
        while buf.len() > max_len {
            buf.pop_front();
        }
        Ok(id)
    }

    async fn xrevrange(&self, key: &str, count: usize) -> Result<Vec<StreamEntry>, anyhow::Error> {
        let streams = self.streams.lock().await;
        Ok(streams
            .get(key)
            .map(|buf| buf.iter().rev().take(count).cloned().collect())
            .unwrap_or_default())
    }
}

/// Redis-backed [`CacheStore`] using a `deadpool-redis` connection pool.
pub struct RedisCacheStore {
    pool: RedisPool,
}

impl RedisCacheStore {
    /// Wraps the given connection pool.
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CacheStore for RedisCacheStore {
    async fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error> {
        let mut conn = self.pool.get().await?;
        let val: Option<String> = conn.get(key).await?;
        Ok(val)
    }

    async fn set(
        &self,
        key: &str,
        value: &str,
        ttl_secs: Option<u64>,
    ) -> Result<(), anyhow::Error> {
        let mut conn = self.pool.get().await?;
        match ttl_secs {
            Some(ttl) => {
                let _: () = conn.set_ex(key, value, ttl).await?;
            }
            None => {
                let _: () = conn.set(key, value).await?;
            }
        }
        Ok(())
    }

    async fn del(&self, key: &str) -> Result<(), anyhow::Error> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(key).await?;
        Ok(())
    }

    async fn set_nx(
        &self,
        key: &str,
        value: &str,
        ttl_secs: Option<u64>,
    ) -> Result<bool, anyhow::Error> {
        let mut conn = self.pool.get().await?;
        let result: bool = match ttl_secs {
            Some(ttl) => deadpool_redis::redis::cmd("SET")
                .arg(key)
                .arg(value)
                .arg("NX")
                .arg("EX")
                .arg(ttl)
                .query_async(&mut *conn)
                .await
                .unwrap_or(false),
            None => conn.set_nx(key, value).await?,
        };
        Ok(result)
    }

    async fn expire(&self, key: &str, ttl_secs: u64) -> Result<(), anyhow::Error> {
        let mut conn = self.pool.get().await?;
        let _: i32 = deadpool_redis::redis::cmd("EXPIRE")
            .arg(key)
            .arg(ttl_secs)
            .query_async(&mut *conn)
            .await?;
        Ok(())
    }

    async fn xadd_maxlen(
        &self,
        key: &str,
        max_len: usize,
        fields: &[(&str, &str)],
    ) -> Result<String, anyhow::Error> {
        let mut conn = self.pool.get().await?;
        let mut cmd = deadpool_redis::redis::cmd("XADD");
        cmd.arg(key).arg("MAXLEN").arg("~").arg(max_len).arg("*");
        for (f, v) in fields {
            cmd.arg(*f).arg(*v);
        }
        let id: String = cmd.query_async(&mut *conn).await?;
        Ok(id)
    }

    async fn xrevrange(&self, key: &str, count: usize) -> Result<Vec<StreamEntry>, anyhow::Error> {
        let mut conn = self.pool.get().await?;
        // Response shape: Vec<(String id, Vec<(String field, String value)>)>
        let raw: Vec<(String, Vec<(String, String)>)> = deadpool_redis::redis::cmd("XREVRANGE")
            .arg(key)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(count)
            .query_async(&mut *conn)
            .await?;
        Ok(raw
            .into_iter()
            .map(|(id, fields)| StreamEntry { id, fields })
            .collect())
    }
}

/// In-memory [`PubSub`] for tests. Messages are fanned out via `tokio::sync::broadcast`.
pub struct InMemoryPubSub {
    senders: Mutex<HashMap<String, Arc<broadcast::Sender<String>>>>,
}

impl InMemoryPubSub {
    /// Creates an empty pub/sub with no channels.
    pub fn new() -> Self {
        Self {
            senders: Mutex::new(HashMap::new()),
        }
    }

    async fn get_or_create_sender(&self, channel: &str) -> Arc<broadcast::Sender<String>> {
        let mut senders = self.senders.lock().await;
        senders
            .entry(channel.to_string())
            .or_insert_with(|| Arc::new(broadcast::channel(256).0))
            .clone()
    }
}

impl Default for InMemoryPubSub {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PubSub for InMemoryPubSub {
    async fn publish(&self, channel: &str, msg: &str) -> Result<(), anyhow::Error> {
        let tx = self.get_or_create_sender(channel).await;
        let _ = tx.send(msg.to_string());
        Ok(())
    }

    async fn subscribe(&self, channel: &str) -> Result<broadcast::Receiver<String>, anyhow::Error> {
        let tx = self.get_or_create_sender(channel).await;
        Ok(tx.subscribe())
    }

    async fn unsubscribe(&self, channel: &str) -> Result<(), anyhow::Error> {
        let mut senders = self.senders.lock().await;
        senders.remove(channel);
        Ok(())
    }
}

/// Pub/sub abstraction for cross-instance event distribution.
#[async_trait]
pub trait PubSub: Send + Sync {
    /// Publishes `msg` to `channel`. Delivery is fire-and-forget.
    async fn publish(&self, channel: &str, msg: &str) -> Result<(), anyhow::Error>;

    /// Subscribes to `channel` and returns a receiver for incoming messages.
    /// Each call yields an independent receiver; all receivers for the same
    /// channel observe the same stream of messages.
    async fn subscribe(&self, channel: &str) -> Result<broadcast::Receiver<String>, anyhow::Error>;

    /// Unsubscribes the shared forwarder for `channel` (if any). Any receivers
    /// returned from previous [`subscribe`](Self::subscribe) calls on this
    /// channel will observe `RecvError::Closed`, letting downstream tasks
    /// exit cleanly. Subsequent `subscribe` calls re-open the channel.
    async fn unsubscribe(&self, channel: &str) -> Result<(), anyhow::Error>;
}

/// Shared per-channel state for [`RedisPubSub`]: the broadcast sender that
/// local subscribers listen on, plus a cancellation token that stops the
/// background Redis forwarder task and releases its `Arc<Sender>` so
/// downstream receivers observe `RecvError::Closed`.
struct RedisChannelHandle {
    sender: Arc<broadcast::Sender<String>>,
    cancel: CancellationToken,
}

/// Redis-backed [`PubSub`]. `PUBLISH` goes through the shared pool; each
/// subscribed channel spawns a dedicated connection in a background task that
/// forwards Redis messages to a `tokio::sync::broadcast` channel shared by all
/// local subscribers.
pub struct RedisPubSub {
    pool: RedisPool,
    redis_url: String,
    channels: Mutex<HashMap<String, RedisChannelHandle>>,
}

impl RedisPubSub {
    /// Creates a new `RedisPubSub` using `pool` for publishes and `redis_url`
    /// when opening dedicated subscription connections.
    pub fn new(pool: RedisPool, redis_url: String) -> Self {
        Self {
            pool,
            redis_url,
            channels: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl PubSub for RedisPubSub {
    async fn publish(&self, channel: &str, msg: &str) -> Result<(), anyhow::Error> {
        let mut conn = self.pool.get().await?;
        let _: () = deadpool_redis::redis::cmd("PUBLISH")
            .arg(channel)
            .arg(msg)
            .query_async(&mut *conn)
            .await?;
        Ok(())
    }

    async fn subscribe(&self, channel: &str) -> Result<broadcast::Receiver<String>, anyhow::Error> {
        let mut channels = self.channels.lock().await;

        if let Some(handle) = channels.get(channel) {
            return Ok(handle.sender.subscribe());
        }

        let (tx, rx) = broadcast::channel(256);
        let tx = Arc::new(tx);
        let cancel = CancellationToken::new();
        channels.insert(
            channel.to_string(),
            RedisChannelHandle {
                sender: tx.clone(),
                cancel: cancel.clone(),
            },
        );

        let client = deadpool_redis::redis::Client::open(self.redis_url.as_str())?;
        let mut pubsub_conn = client.get_async_pubsub().await?;
        pubsub_conn.subscribe(channel).await?;

        let channel_name = channel.to_string();
        let mut msg_stream = pubsub_conn.into_on_message();
        tokio::spawn(async move {
            use tokio_stream::StreamExt as _;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!(channel = %channel_name, "Redis PubSub forwarder cancelled");
                        break;
                    }
                    next = msg_stream.next() => {
                        match next {
                            Some(msg) => {
                                if let Ok(payload) = msg.get_payload::<String>() {
                                    let _ = tx.send(payload);
                                }
                            }
                            None => {
                                tracing::warn!(channel = %channel_name, "Redis PubSub stream ended");
                                break;
                            }
                        }
                    }
                }
            }
            // Dropping `tx` here releases this task's Arc<Sender>; together
            // with the map entry removed by `unsubscribe`, all receivers then
            // observe RecvError::Closed.
            drop(tx);
        });

        Ok(rx)
    }

    async fn unsubscribe(&self, channel: &str) -> Result<(), anyhow::Error> {
        let mut channels = self.channels.lock().await;
        if let Some(handle) = channels.remove(channel) {
            handle.cancel.cancel();
        }
        Ok(())
    }
}
