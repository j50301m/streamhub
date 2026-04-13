use std::collections::HashMap;

use async_trait::async_trait;
use deadpool_redis::Pool as RedisPool;
use deadpool_redis::redis::AsyncCommands;
use tokio::sync::Mutex;

/// Key-value cache abstraction with optional TTL.
#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error>;
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>)
    -> Result<(), anyhow::Error>;
    async fn del(&self, key: &str) -> Result<(), anyhow::Error>;
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
}
