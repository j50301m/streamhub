# SPEC-009 GCS 上傳

狀態：review

## 目標

直播結束後，本地 ffmpeg 轉 HLS，再上傳 HLS 檔案到 GCS。VOD 播放從 GCS 讀取。
本地開發用 fake-gcs-server 模擬 GCS。

## 影響範圍

新增 / 修改：
- `crates/storage/` — GCS 上傳封裝（trait-based，方便 mock）
- `crates/common/src/config.rs` — GCS 設定（bucket、credentials、enabled）
- `crates/common/src/lib.rs` — AppState 加 storage
- `crates/api/src/handlers/recording.rs` — 轉檔後上傳 HLS 到 GCS
- `deploy/docker-compose.yml` — 加 fake-gcs-server 容器
- `web/viewer/index.html` — VOD hls_url 可能指向 GCS 公開 URL

## Todo list

- [x] SPEC-009-01 storage crate — GcsStorage trait + 實作（upload_file、upload_dir、get_public_url）
- [x] SPEC-009-02 Config — GCS_BUCKET、GCS_CREDENTIALS_PATH、GCS_ENDPOINT（本地用 fake-gcs）、STORAGE_ENABLED
- [x] SPEC-009-03 docker-compose — 加 fake-gcs-server 容器，port 4443
- [x] SPEC-009-04 AppState — 加 Option<GcsStorage>（STORAGE_ENABLED=false 時為 None）
- [x] SPEC-009-05 recording handler — run_transcode 完成後，若 storage enabled，上傳 HLS 目錄到 GCS，hls_url 改為 GCS URL
- [x] SPEC-009-06 上傳後可選清理本地 HLS 檔案
- [x] SPEC-009-07 unit test — mock storage trait 測試上傳流程
- [x] SPEC-009-08 驗證 — cargo build + test + clippy + fmt

## 架構設計

### Storage trait

```rust
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    async fn upload_file(&self, local_path: &Path, gcs_key: &str) -> Result<String, StorageError>;
    async fn upload_dir(&self, local_dir: &Path, gcs_prefix: &str) -> Result<Vec<String>, StorageError>;
    fn public_url(&self, gcs_key: &str) -> String;
}
```

### GCS 實作

使用 `google-cloud-storage` crate 或 `object_store` crate。

### 上傳流程

```
recording hook → ffmpeg 轉 HLS（本地）
    → upload_dir("/recordings/{key}/hls/", "streams/{key}/hls/")
    → hls_url = storage.public_url("streams/{key}/hls/index.m3u8")
    → 可選清理本地 /recordings/{key}/hls/
```

### fake-gcs-server

```yaml
fake-gcs:
  image: fsouza/fake-gcs-server
  container_name: streamhub-fake-gcs
  ports:
    - "4443:4443"
  command: ["-scheme", "http", "-port", "4443", "-public-host", "localhost:4443"]
  volumes:
    - gcs-data:/storage
```

### Config

```rust
pub struct AppConfig {
    // ... existing
    pub gcs_bucket: String,           // default: streamhub-recordings-dev
    pub gcs_endpoint: String,         // default: "" (empty = real GCS, non-empty = fake)
    pub gcs_credentials_path: String, // default: "" (empty = ADC)
    pub storage_enabled: bool,        // default: false
}
```

## 驗收流程

```bash
cd deploy && docker compose up --build -d
# fake-gcs-server 啟動在 :4443
# 設 STORAGE_ENABLED=true, GCS_ENDPOINT=http://fake-gcs:4443

# 推流 → 停止 → 確認 HLS 上傳到 fake-gcs
# curl http://localhost:4443/storage/v1/b/streamhub-recordings-dev/o 查看物件
```

## 備註

- fake-gcs-server 完全相容 GCS JSON API，切換到 real GCS 只需移除 GCS_ENDPOINT
- storage_enabled=false 時行為和現在一樣（本地 HLS，nginx serve）
- 上傳用 stream_key 作為 GCS prefix：`streams/{stream_key}/hls/`
- public_url 格式：real GCS 用 `https://storage.googleapis.com/{bucket}/...`，fake 用 `http://localhost:4443/{bucket}/...`
