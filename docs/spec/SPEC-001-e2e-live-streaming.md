# SPEC-001 End-to-End Live Streaming MVP

狀態：done

## 目標

建立可運作的端到端直播系統：直播主從瀏覽器推流（WebRTC WHIP），觀眾透過 WHEP 或 HLS 觀看。
本輪不含認證、錄影上傳、VOD 轉檔，僅做核心串流功能。

## 影響範圍

新增：
- `Cargo.toml`（workspace root）
- `crates/common/` — AppError、Config（cfgloader_rs）、DB pool、AppState
- `crates/entity/` — streams entity（dense format，schema source of truth）
- `crates/migration/` — seed data 專用（schema 由 API 啟動時 entity sync 處理）
- `crates/hook/` — MediaMTX webhook handler
- `crates/api/` — Axum HTTP server（啟動時自動 sync entity schema）
- `crates/auth/` — 骨架（空 crate）
- `crates/stream/` — 骨架（空 crate）
- `crates/storage/` — 骨架（空 crate）
- `crates/transcoder/` — 骨架（空 crate）
- `deploy/services/docker-compose.yml` — PostgreSQL 17（port 5433）+ MediaMTX
- `deploy/services/mediamtx.yml` — WHIP/WHEP + webhook 設定
- `web/broadcaster/index.html` — 推流頁面
- `web/viewer/index.html` — 觀看頁面

## Todo list

- [x] SPEC-001-01 Cargo workspace + 所有 crate 骨架 + docker-compose.yml（PG 17 + MediaMTX）
- [x] SPEC-001-02 common crate — AppError（thiserror）、Config（cfgloader_rs）、DB pool（statement_timeout 30s）
- [x] SPEC-001-03 entity crate — streams entity（dense format：id, stream_key, title, status ActiveEnum, started_at, ended_at, created_at）
- [x] SPEC-001-04 API 啟動時用 get_schema_registry("entity::*").sync() 自動同步 schema
- [x] SPEC-001-05 api crate — Axum 骨架 + GET /healthz + POST /v1/streams + GET /v1/streams/:id
- [x] SPEC-001-06 hook crate + POST /internal/hooks/publish（publish/unpublish → 更新 stream status）
- [x] SPEC-001-07 deploy/services/mediamtx.yml — 設定 WHIP/WHEP path、webhook 指向 API、錄製暫時關閉
- [x] SPEC-001-08 web/broadcaster/index.html — getUserMedia → WHIP 推流
- [x] SPEC-001-09 web/viewer/index.html — WHEP 拉流 + hls.js fallback

## 驗收流程

### 前置準備

```bash
# 1. 啟動 PostgreSQL + MediaMTX
docker compose -f deploy/services/docker-compose.yml up -d

# 2. 確認容器正常
docker ps  # 應看到 streamhub-postgres (healthy) + streamhub-mediamtx

# 3. 啟動 API server（schema 會自動 sync）
cargo run -p api
# 應看到：
#   INFO Connecting to database...
#   INFO Syncing database schema from entities...
#   INFO Starting server on 0.0.0.0:8080
```

### 驗收 1-3：基礎設施

```bash
# 另開 terminal

# healthz
curl localhost:8080/healthz
# 預期：{"status":"ok"}

# 建立串流
curl -s -X POST localhost:8080/v1/streams \
  -H "Content-Type: application/json" \
  -d '{"title":"test stream"}' | python3 -m json.tool
# 預期：{"data":{"id":"...","stream_key":"...","status":"Pending",...}}

# 記下回傳的 stream_key（UUID），後面要用
```

### 驗收 4-5：推流

1. 瀏覽器開啟 `web/broadcaster/index.html`
2. API URL 保持 `http://localhost:8080`
3. 輸入 title，按 **Create Stream**
4. 按 **Start Streaming**（瀏覽器會要求攝影機權限，允許）
5. 應看到本地攝影機預覽畫面，status 顯示 connected

驗證 webhook：

```bash
# 查 stream 狀態，應該變成 Live
curl -s localhost:8080/v1/streams/{stream_key} | python3 -m json.tool
# 預期："status": "Live", "started_at": "2026-..."
```

### 驗收 6：觀看

1. 另開瀏覽器 tab 打開 `web/viewer/index.html`
2. 輸入同一個 **stream_key**
3. 按 **Watch (WHEP)** — 應看到即時畫面（延遲 < 1s）
4. 或按 **Watch (HLS)** — 應看到畫面（延遲 2-3s）

### 驗收 7：停止推流

1. 回到 broadcaster 頁面，按 **Stop Streaming**
2. 驗證：

```bash
curl -s localhost:8080/v1/streams/{stream_key} | python3 -m json.tool
# 預期："status": "Ended", "ended_at": "2026-..."
```

### 驗收 8：程式碼品質

```bash
cargo build && cargo test && cargo clippy -- -D warnings && cargo fmt --check
# 全部 pass
```

### 清理

```bash
docker compose -f deploy/services/docker-compose.yml down
```

## 備註

- 不含 auth，所有 API 端點暫時無認證
- 不含 users / recordings / stream_tokens 表，後續 spec 再加
- MediaMTX 錄製功能本輪關閉（record: no）
- Entity 是 schema source of truth，API 啟動時透過 `get_schema_registry("entity::*").sync()` 自動同步
- Migration crate 保留給 seed data insert，手動執行 `cargo run -p migration`
- Config 使用 cfgloader_rs（#[derive(FromEnv)]），支援 .env 檔 + 環境變數 + 預設值
- 本地 PostgreSQL port 為 5433（避免與其他服務衝突）
