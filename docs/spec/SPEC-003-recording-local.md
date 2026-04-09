# SPEC-003 錄影功能（本地版）

狀態：in-progress

## 目標

直播結束後自動產生錄影檔，透過 API 可查詢錄影記錄。
本輪錄影存本地，GCS 上傳和 Transcoder 轉檔留後續 spec。

## 影響範圍

新增 / 修改：
- `crates/entity/src/recording.rs` — recordings entity
- `crates/entity/src/stream.rs` — 加 vod_status ActiveEnum + hls_url
- `crates/hook/src/recording.rs` — 錄影完成 webhook handler
- `crates/api/src/routes/streams.rs` — GET /v1/streams/:id/recordings
- `crates/api/src/routes/streams.rs` — stream response 加 vod_status
- `deploy/mediamtx.yml` — 開啟錄製 + runOnRecordSegmentComplete webhook
- `deploy/docker-compose.yml` — recordings volume mount

## Todo list

- [x] SPEC-003-01 entity — recordings（id, stream_id FK, file_path, duration_secs Option, file_size_bytes Option, created_at）
- [x] SPEC-003-02 entity — streams 加 vod_status ActiveEnum（None/Processing/Ready/Failed）
- [x] SPEC-003-03 hook — POST /internal/hooks/recording handler（收到 MediaMTX webhook → 讀檔案大小 → 建 recording 記錄）
- [x] SPEC-003-04 mediamtx.yml — 開啟 record: yes、recordFormat: fmp4、recordPath、runOnRecordSegmentComplete webhook 指向 API
- [x] SPEC-003-05 docker-compose.yml — 加 recordings volume mount 給 MediaMTX + API 共用
- [x] SPEC-003-06 API — GET /v1/streams/:id/recordings（列出該 stream 的錄影，需認證 owner）
- [x] SPEC-003-07 API — stream response 加 vod_status 欄位
- [x] SPEC-003-08 hook — unpublish 時自動更新 vod_status = Ready（本地版簡化，不經 transcoder）

## 驗收流程

### 前置準備

```bash
docker compose -f deploy/docker-compose.yml up -d
cargo run -p api
```

### 測試錄影

```bash
# 1. 註冊 + 登入
curl -s -X POST localhost:8080/v1/auth/register \
  -H "Content-Type: application/json" \
  -d '{"email":"rec@test.com","password":"test1234","role":"Broadcaster"}' | python3 -m json.tool

# 2. 建立串流
curl -s -X POST localhost:8080/v1/streams \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer {access_token}" \
  -d '{"title":"recording test"}' | python3 -m json.tool

# 3. 取得推流 token + 推流（broadcaster 頁面）
# 推流一段時間後停止

# 4. 查 stream — vod_status 應為 Ready
curl -s localhost:8080/v1/streams/{stream_id} | python3 -m json.tool
# 預期："vod_status": "Ready"

# 5. 查錄影列表
curl -s localhost:8080/v1/streams/{stream_id}/recordings \
  -H "Authorization: Bearer {access_token}" | python3 -m json.tool
# 預期：至少一筆 recording，有 file_path + file_size_bytes
```

### 程式碼品質

```bash
cargo build && cargo test && cargo clippy -- -D warnings && cargo fmt --check
```

## 備註

- 錄影檔存本地 `/recordings/{stream_key}/` 目錄，MediaMTX 和 API 共用 volume
- file_path 存相對路徑（如 `/recordings/{stream_key}/2026-04-09_14-00-00.fmp4`）
- duration_secs 和 file_size_bytes 可能 MediaMTX webhook 不提供完整資訊，從檔案系統讀取
- vod_status 本輪簡化：unpublish 後直接標 Ready（跳過 Processing），後續接 Transcoder 再改
- GCS 上傳和 Transcoder API 留給 SPEC-004
