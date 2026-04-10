# SPEC-010 GCP Transcoder API

狀態：done

## 目標

正式環境用 GCP Transcoder API 將 MP4 轉成多解析度 ABR HLS。
本地開發保留 ffmpeg 本地轉檔（TRANSCODER_ENABLED=false）。

## 影響範圍

修改：
- `crates/transcoder/` — 加 GCP Transcoder API 呼叫（REST via reqwest）
- `crates/api/src/handlers/recording.rs` — run_transcode 根據 flag 走不同路徑
- `crates/common/src/config.rs` — 加 TRANSCODER_ENABLED、TRANSCODER_LOCATION、TRANSCODER_PROJECT_ID
- `crates/api/src/routes.rs` — 加 Pub/Sub webhook endpoint

新增：
- `crates/api/src/handlers/transcoder_webhook.rs` — Pub/Sub push 通知 handler

## Todo list

- [x] SPEC-010-01 Config — TRANSCODER_ENABLED、TRANSCODER_PROJECT_ID、TRANSCODER_LOCATION、PUBSUB_VERIFY_TOKEN
- [x] SPEC-010-02 transcoder crate — create_job()：呼叫 GCP Transcoder API REST endpoint，輸入 GCS MP4 路徑，輸出多解析度 HLS
- [x] SPEC-010-03 recording handler — TRANSCODER_ENABLED 時：上傳 MP4 到 GCS → create_job()，不做本地 ffmpeg
- [x] SPEC-010-04 Pub/Sub webhook — POST /internal/hooks/transcoder-complete，收到 job 完成通知 → 更新 vod_status=Ready + hls_url
- [x] SPEC-010-05 routes.rs — 註冊 transcoder webhook route
- [x] SPEC-010-06 驗證 — cargo build + test + clippy + fmt

## 架構設計

### 流程對比

```
TRANSCODER_ENABLED=false（本地，現有邏輯不動）：
  unpublish → recording hook → ffmpeg 轉 HLS → 上傳 HLS 到 GCS → vod_status=Ready

TRANSCODER_ENABLED=true（正式環境）：
  unpublish → recording hook → 上傳 MP4 到 GCS → Transcoder API create job → vod_status=Processing
  ... Transcoder 處理中 ...
  Pub/Sub → POST /internal/hooks/transcoder-complete → vod_status=Ready + hls_url
```

### Transcoder API

- 3 video streams: 1080p (5Mbps), 720p (2.5Mbps), 360p (1Mbps)
- 1 audio stream: AAC 128kbps
- Output: ABR HLS manifest (index.m3u8)
- Pub/Sub notification on completion
- stream_id in job labels for webhook lookup

## 備註

- TRANSCODER_ENABLED 預設 false，本地開發不影響
- Auth 用 gcloud CLI token（TODO: 正式環境用 Workload Identity）
- Pub/Sub push subscription 需在 GCP console 設定
