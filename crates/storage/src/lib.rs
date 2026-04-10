use async_trait::async_trait;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("upload failed: {0}")]
    Upload(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait ObjectStorage: Send + Sync {
    /// Upload a single file, returns the GCS key.
    async fn upload_file(&self, local_path: &Path, gcs_key: &str) -> Result<String, StorageError>;

    /// Upload all files in a directory (non-recursive), returns list of GCS keys.
    async fn upload_dir(
        &self,
        local_dir: &Path,
        gcs_prefix: &str,
    ) -> Result<Vec<String>, StorageError>;

    /// Get the public URL for a GCS key.
    fn public_url(&self, gcs_key: &str) -> String;
}

pub struct GcsStorage {
    store: object_store::gcp::GoogleCloudStorage,
    bucket: String,
    endpoint: Option<String>,
    public_base_url: String,
}

impl GcsStorage {
    pub fn new(
        bucket: &str,
        endpoint: Option<&str>,
        credentials_path: Option<&str>,
    ) -> Result<Self, StorageError> {
        let mut builder =
            object_store::gcp::GoogleCloudStorageBuilder::new().with_bucket_name(bucket);

        if let Some(ep) = endpoint {
            // Custom endpoint (e.g. fake-gcs-server): set base URL, skip signature,
            // and provide a fake service account key to disable OAuth token lookup.
            let fake_key = r#"{"private_key": "private_key", "private_key_id": "id", "client_email": "fake@example.com", "disable_oauth": true}"#;
            builder = builder
                .with_config(object_store::gcp::GoogleConfigKey::BaseUrl, ep)
                .with_config(object_store::gcp::GoogleConfigKey::SkipSignature, "true")
                .with_service_account_key(fake_key);
        }
        if let Some(creds) = credentials_path {
            builder = builder.with_service_account_path(creds);
        }

        let store = builder
            .build()
            .map_err(|e| StorageError::Upload(e.to_string()))?;

        let public_base_url = match endpoint {
            Some(ep) => format!("{}/{}", ep.trim_end_matches('/'), bucket),
            None => format!("https://storage.googleapis.com/{}", bucket),
        };

        Ok(Self {
            store,
            bucket: bucket.to_string(),
            endpoint: endpoint.map(|s| s.to_string()),
            public_base_url,
        })
    }

    /// Ensure the bucket exists (for fake-gcs-server).
    /// On real GCS this is a no-op since bucket should be pre-created.
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        if let Some(ep) = &self.endpoint {
            let url = format!("{}/storage/v1/b", ep.trim_end_matches('/'));
            let body = format!(r#"{{"name":"{}"}}"#, self.bucket);
            let client = reqwest::Client::new();
            let resp = client.post(&url)
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
                .map_err(|e| StorageError::Upload(e.to_string()))?;
            if resp.status().is_success() || resp.status().as_u16() == 409 {
                // 409 = bucket already exists, that's fine
                tracing::info!(bucket = %self.bucket, "GCS bucket ensured");
            } else {
                tracing::warn!(bucket = %self.bucket, status = %resp.status(), "Failed to create bucket");
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for GcsStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsStorage")
            .field("public_base_url", &self.public_base_url)
            .finish()
    }
}

#[async_trait]
impl ObjectStorage for GcsStorage {
    async fn upload_file(&self, local_path: &Path, gcs_key: &str) -> Result<String, StorageError> {
        use object_store::ObjectStoreExt as _;

        let data = tokio::fs::read(local_path).await?;
        let path = object_store::path::Path::from(gcs_key);
        self.store
            .put(&path, data.into())
            .await
            .map_err(|e| StorageError::Upload(e.to_string()))?;

        tracing::info!(gcs_key, "Uploaded file to GCS");
        Ok(gcs_key.to_string())
    }

    async fn upload_dir(
        &self,
        local_dir: &Path,
        gcs_prefix: &str,
    ) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        let mut entries = tokio::fs::read_dir(local_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_file() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let gcs_key = format!("{}/{}", gcs_prefix.trim_end_matches('/'), file_name);
                self.upload_file(&entry.path(), &gcs_key).await?;
                keys.push(gcs_key);
            }
        }
        tracing::info!(
            dir = %local_dir.display(),
            prefix = gcs_prefix,
            count = keys.len(),
            "Uploaded directory to GCS"
        );
        Ok(keys)
    }

    fn public_url(&self, gcs_key: &str) -> String {
        format!("{}/{}", self.public_base_url, gcs_key)
    }
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
        async fn upload_file(
            &self,
            _local_path: &Path,
            gcs_key: &str,
        ) -> Result<String, StorageError> {
            self.uploaded.lock().unwrap().push(gcs_key.to_string());
            Ok(gcs_key.to_string())
        }

        async fn upload_dir(
            &self,
            local_dir: &Path,
            gcs_prefix: &str,
        ) -> Result<Vec<String>, StorageError> {
            let mut keys = Vec::new();
            let mut entries = tokio::fs::read_dir(local_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                if entry.file_type().await?.is_file() {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let gcs_key = format!("{}/{}", gcs_prefix.trim_end_matches('/'), file_name);
                    self.upload_file(&entry.path(), &gcs_key).await?;
                    keys.push(gcs_key);
                }
            }
            Ok(keys)
        }

        fn public_url(&self, gcs_key: &str) -> String {
            format!("http://mock-gcs/{}", gcs_key)
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
        assert_eq!(url, "http://mock-gcs/streams/abc/hls/index.m3u8");
    }
}
