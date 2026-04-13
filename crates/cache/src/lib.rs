use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use deadpool_redis::Pool as RedisPool;
use deadpool_redis::redis::AsyncCommands;
use tokio::sync::{Mutex, broadcast};

/// Key-value cache abstraction with optional TTL.
#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error>;
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>)
    -> Result<(), anyhow::Error>;
    async fn del(&self, key: &str) -> Result<(), anyhow::Error>;

    /// SET if Not eXists. Returns true if key was set, false if already exists.
    async fn set_nx(
        &self,
        key: &str,
        value: &str,
        ttl_secs: Option<u64>,
    ) -> Result<bool, anyhow::Error>;
}

/// In-memory CacheStore using a HashMap (no TTL enforcement).
pub struct InMemoryCache {
    data: Mutex<HashMap<String, String>>,
}

impl InMemoryCache {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
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
}

/// Redis-backed CacheStore using deadpool-redis.
pub struct RedisCacheStore {
    pool: RedisPool,
}

impl RedisCacheStore {
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
}

/// In-memory PubSub for testing (messages are broadcast locally).
pub struct InMemoryPubSub {
    senders: Mutex<HashMap<String, Arc<broadcast::Sender<String>>>>,
}

impl InMemoryPubSub {
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
}

/// Pub/Sub abstraction for cross-instance event distribution.
#[async_trait]
pub trait PubSub: Send + Sync {
    /// Publish a message to a channel.
    async fn publish(&self, channel: &str, msg: &str) -> Result<(), anyhow::Error>;

    /// Subscribe to a channel. Returns a broadcast receiver for incoming messages.
    async fn subscribe(&self, channel: &str) -> Result<broadcast::Receiver<String>, anyhow::Error>;
}

/// Redis-backed PubSub using deadpool-redis for PUBLISH and a dedicated connection for SUBSCRIBE.
pub struct RedisPubSub {
    pool: RedisPool,
    redis_url: String,
    /// Shared broadcast senders per channel (lazily created on first subscribe).
    senders: Mutex<HashMap<String, Arc<broadcast::Sender<String>>>>,
}

impl RedisPubSub {
    pub fn new(pool: RedisPool, redis_url: String) -> Self {
        Self {
            pool,
            redis_url,
            senders: Mutex::new(HashMap::new()),
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
        let mut senders = self.senders.lock().await;

        if let Some(tx) = senders.get(channel) {
            return Ok(tx.subscribe());
        }

        // Create a new broadcast channel (buffer 256 messages)
        let (tx, rx) = broadcast::channel(256);
        let tx = Arc::new(tx);
        senders.insert(channel.to_string(), tx.clone());

        // Spawn a dedicated connection for SUBSCRIBE (cannot use pool — blocking op)
        let client = deadpool_redis::redis::Client::open(self.redis_url.as_str())?;
        let mut pubsub_conn = client.get_async_pubsub().await?;
        pubsub_conn.subscribe(channel).await?;

        let channel_name = channel.to_string();
        let mut msg_stream = pubsub_conn.into_on_message();
        tokio::spawn(async move {
            use tokio_stream::StreamExt as _;
            while let Some(msg) = msg_stream.next().await {
                if let Ok(payload) = msg.get_payload::<String>() {
                    // If all receivers dropped, the send will fail — that's fine
                    let _ = tx.send(payload);
                }
            }
            tracing::warn!(channel = %channel_name, "Redis PubSub stream ended");
        });

        Ok(rx)
    }
}
