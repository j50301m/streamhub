# SPEC-001 End-to-End Live Streaming MVP

狀態：in-progress

## 目標

建立可運作的端到端直播系統：直播主從瀏覽器推流（WebRTC WHIP），觀眾透過 WHEP 或 HLS 觀看。
本輪不含認證、錄影上傳、VOD 轉檔，僅做核心串流功能。

## 影響範圍

新增：
- `Cargo.toml`（workspace root）
- `crates/common/` — AppError、Config、DB pool
- `crates/entity/` — streams entity（dense format）
- `crates/migration/` — create_table_from_entity 自動建表
- `crates/hook/` — MediaMTX webhook handler
- `crates/api/` — Axum HTTP server
- `crates/auth/` — 骨架（空 crate）
- `crates/stream/` — 骨架（空 crate）
- `crates/storage/` — 骨架（空 crate）
- `crates/transcoder/` — 骨架（空 crate）
- `deploy/docker-compose.yml` — PostgreSQL 17 + MediaMTX
- `deploy/mediamtx.yml` — WHIP/WHEP + webhook 設定
- `web/broadcaster/index.html` — 推流頁面
- `web/viewer/index.html` — 觀看頁面

文件同步：
- [ ] docs/architecture.md（無異動）
- [ ] docs/api.md（無異動，本輪 API 是 api.md 的子集）

## Todo list

- [x] SPEC-001-01 Cargo workspace + 所有 crate 骨架 + docker-compose.yml（PG 17 + MediaMTX）
- [x] SPEC-001-02 common crate — AppError（thiserror）、Config（config crate + 環境變數）、DB pool 初始化（SeaORM ConnectOptions + statement_timeout）
- [x] SPEC-001-03 entity crate — streams entity（dense format：id, stream_key, title, status ActiveEnum, started_at, ended_at, created_at）
- [x] SPEC-001-04 migration crate — 用 Schema::create_table_from_entity(stream::Entity) 自動建表
- [x] SPEC-001-05 api crate — Axum 骨架 + GET /healthz + POST /v1/streams + GET /v1/streams/:id
- [x] SPEC-001-06 hook crate + POST /internal/hooks/publish（publish/unpublish → 更新 stream status）
- [x] SPEC-001-07 deploy/mediamtx.yml — 設定 WHIP/WHEP path、webhook 指向 API、錄製暫時關閉
- [x] SPEC-001-08 web/broadcaster/index.html — getUserMedia → WHIP 推流
- [x] SPEC-001-09 web/viewer/index.html — WHEP 拉流 + hls.js fallback

## 驗收標準

1. `docker compose up -d` 啟動 PostgreSQL + MediaMTX
2. `cargo run -p migration -- up` 自動建 streams 表
3. `cargo run -p api` 啟動 server on :8080
4. `curl -X POST localhost:8080/v1/streams` 回傳 stream 物件（status: Pending）
5. 開啟 broadcaster 頁面，輸入 stream_key 推流 → webhook 觸發 → status 變 Live
6. 開啟 viewer 頁面，輸入 stream_key → 看到即時畫面（WHEP 或 HLS）
7. 停止推流 → webhook 觸發 → status 變 Ended
8. `cargo build && cargo test && cargo clippy -- -D warnings && cargo fmt --check` 全過

## 備註

- 不含 auth，所有 API 端點暫時無認證
- 不含 users / recordings / stream_tokens 表，後續 spec 再加
- MediaMTX 錄製功能本輪關閉（record: no）
- entity 是 schema source of truth，migration 透過 create_table_from_entity 自動產生 DDL
