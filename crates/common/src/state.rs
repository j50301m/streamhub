use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::{Mutex, broadcast};

/// Publish/subscribe abstraction for inter-service communication.
/// In-memory implementation uses tokio broadcast channels.
/// Redis implementation will be added in SPEC-017.
#[async_trait]
pub trait PubSub: Send + Sync {
    async fn publish(&self, channel: &str, msg: &str) -> Result<(), anyhow::Error>;
    async fn subscribe(&self, channel: &str) -> Result<broadcast::Receiver<String>, anyhow::Error>;
}

/// Key-value cache abstraction with optional TTL.
/// In-memory implementation uses a HashMap.
/// Redis implementation will be added in SPEC-017.
#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error>;
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>)
    -> Result<(), anyhow::Error>;
    async fn del(&self, key: &str) -> Result<(), anyhow::Error>;
}

/// In-memory PubSub using tokio broadcast channels.
pub struct InMemoryPubSub {
    channels: Mutex<HashMap<String, broadcast::Sender<String>>>,
}

impl InMemoryPubSub {
    pub fn new() -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
        }
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
        let channels = self.channels.lock().await;
        if let Some(tx) = channels.get(channel) {
            // Ignore error if no receivers
            let _ = tx.send(msg.to_string());
        }
        Ok(())
    }

    async fn subscribe(&self, channel: &str) -> Result<broadcast::Receiver<String>, anyhow::Error> {
        let mut channels = self.channels.lock().await;
        let tx = channels
            .entry(channel.to_string())
            .or_insert_with(|| broadcast::channel(256).0);
        Ok(tx.subscribe())
    }
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
