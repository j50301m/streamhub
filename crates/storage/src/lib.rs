use async_trait::async_trait;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("upload failed: {0}")]
    Upload(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Object storage abstraction. Handlers only depend on this trait.
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    /// Upload a single file, returns the storage key.
    async fn upload_file(&self, local_path: &Path, key: &str) -> Result<String, StorageError>;

    /// Upload all files in a directory (non-recursive), returns list of keys.
    async fn upload_dir(&self, local_dir: &Path, prefix: &str)
    -> Result<Vec<String>, StorageError>;

    /// Get the public URL for a storage key.
    fn public_url(&self, key: &str) -> String;
}

/// GCS implementation using JSON API via reqwest.
/// Works with both real GCS and fake-gcs-server.
pub struct GcsStorage {
    client: reqwest::Client,
    bucket: String,
    base_url: String,
    public_base_url: String,
    auth_token: Option<String>,
}

impl GcsStorage {
    /// Create a new GCS storage client.
    ///
    /// - `bucket`: GCS bucket name
    /// - `endpoint`: Custom endpoint URL (e.g. `http://fake-gcs:4443` for local dev).
    ///   If None, uses the real GCS endpoint.
    /// - `credentials_path`: Path to service account JSON key file.
    ///   If None, uses Application Default Credentials (ADC).
    pub async fn new(
        bucket: &str,
        endpoint: Option<&str>,
        credentials_path: Option<&str>,
    ) -> Result<Self, StorageError> {
        let base_url = endpoint
            .unwrap_or("https://storage.googleapis.com")
            .trim_end_matches('/')
            .to_string();

        let public_base_url = if endpoint.is_some() {
            // Local dev: serve via nginx /gcs/ proxy
            format!("/gcs/{}", bucket)
        } else {
            // Real GCS: public URL
            format!("https://storage.googleapis.com/{}", bucket)
        };

        // Load auth token for real GCS (skip for fake-gcs)
        let auth_token = if endpoint.is_none() {
            load_access_token(credentials_path).await.ok()
        } else {
            None
        };

        Ok(Self {
            client: reqwest::Client::new(),
            bucket: bucket.to_string(),
            base_url,
            public_base_url,
            auth_token,
        })
    }

    /// Ensure the bucket exists (for fake-gcs-server).
    /// On real GCS this is a no-op since the bucket should be pre-created.
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        let url = format!("{}/storage/v1/b", self.base_url);
        let body = format!(r#"{{"name":"{}"}}"#, self.bucket);
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| StorageError::Upload(e.to_string()))?;
        // 200 = created, 409 = already exists — both fine
        if resp.status().is_success() || resp.status().as_u16() == 409 {
            tracing::info!(bucket = %self.bucket, "GCS bucket ensured");
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(bucket = %self.bucket, %status, %body, "Failed to ensure bucket");
        }
        Ok(())
    }

    fn upload_url(&self, key: &str) -> String {
        format!(
            "{}/upload/storage/v1/b/{}/o?uploadType=media&name={}",
            self.base_url, self.bucket, key
        )
    }
}

impl std::fmt::Debug for GcsStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsStorage")
            .field("bucket", &self.bucket)
            .field("base_url", &self.base_url)
            .finish()
    }
}

#[async_trait]
impl ObjectStorage for GcsStorage {
    #[tracing::instrument(skip(self), fields(bucket = %self.bucket, %key))]
    async fn upload_file(&self, local_path: &Path, key: &str) -> Result<String, StorageError> {
        let data = tokio::fs::read(local_path).await?;
        let url = self.upload_url(key);

        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(data);

        if let Some(token) = &self.auth_token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StorageError::Upload(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StorageError::Upload(format!("{status}: {body}")));
        }

        tracing::info!(key, "Uploaded file to storage");
        Ok(key.to_string())
    }

    #[tracing::instrument(skip(self), fields(bucket = %self.bucket, %prefix))]
    async fn upload_dir(
        &self,
        local_dir: &Path,
        prefix: &str,
    ) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        let mut entries = tokio::fs::read_dir(local_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_file() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let key = format!("{}/{}", prefix.trim_end_matches('/'), file_name);
                self.upload_file(&entry.path(), &key).await?;
                keys.push(key);
            }
        }
        tracing::info!(
            dir = %local_dir.display(),
            prefix,
            count = keys.len(),
            "Uploaded directory to storage"
        );
        Ok(keys)
    }

    fn public_url(&self, key: &str) -> String {
        format!("{}/{}", self.public_base_url, key)
    }
}

/// Load a GCS access token from service account credentials or ADC.
/// This is a simplified implementation — for production, use a token refresh mechanism.
async fn load_access_token(_credentials_path: Option<&str>) -> Result<String, StorageError> {
    // TODO: Implement proper token loading from service account JSON or metadata server.
    // For now, try the gcloud CLI token as a fallback.
    let output = tokio::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .await
        .map_err(|e| StorageError::Upload(format!("failed to get access token: {e}")))?;

    if !output.status.success() {
        return Err(StorageError::Upload(
            "gcloud auth print-access-token failed".to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub struct MockStorage {
        pub uploaded: Arc<Mutex<Vec<String>>>,
    }

    impl MockStorage {
        pub fn new() -> Self {
            Self {
                uploaded: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ObjectStorage for MockStorage {
        async fn upload_file(&self, _local_path: &Path, key: &str) -> Result<String, StorageError> {
            self.uploaded.lock().unwrap().push(key.to_string());
            Ok(key.to_string())
        }

        async fn upload_dir(
            &self,
            local_dir: &Path,
            prefix: &str,
        ) -> Result<Vec<String>, StorageError> {
            let mut keys = Vec::new();
            let mut entries = tokio::fs::read_dir(local_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                if entry.file_type().await?.is_file() {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let key = format!("{}/{}", prefix.trim_end_matches('/'), file_name);
                    self.upload_file(&entry.path(), &key).await?;
                    keys.push(key);
                }
            }
            Ok(keys)
        }

        fn public_url(&self, key: &str) -> String {
            format!("http://mock-storage/{}", key)
        }
    }

    #[tokio::test]
    async fn test_mock_upload_file() {
        let storage = MockStorage::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"test data").unwrap();

        let key = storage
            .upload_file(tmp.path(), "streams/abc/hls/index.m3u8")
            .await
            .unwrap();

        assert_eq!(key, "streams/abc/hls/index.m3u8");
        assert_eq!(
            storage.uploaded.lock().unwrap().as_slice(),
            &["streams/abc/hls/index.m3u8"]
        );
    }

    #[tokio::test]
    async fn test_mock_upload_dir() {
        let storage = MockStorage::new();
        let tmp_dir = tempfile::tempdir().unwrap();
        std::fs::write(tmp_dir.path().join("index.m3u8"), b"#EXTM3U").unwrap();
        std::fs::write(tmp_dir.path().join("seg0.ts"), b"segment").unwrap();

        let keys = storage
            .upload_dir(tmp_dir.path(), "streams/abc/hls")
            .await
            .unwrap();

        assert_eq!(keys.len(), 2);
        let uploaded = storage.uploaded.lock().unwrap();
        assert_eq!(uploaded.len(), 2);
    }

    #[test]
    fn test_mock_public_url() {
        let storage = MockStorage::new();
        let url = storage.public_url("streams/abc/hls/index.m3u8");
        assert_eq!(url, "http://mock-storage/streams/abc/hls/index.m3u8");
    }
}
