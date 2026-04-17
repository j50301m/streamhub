# SPEC-034 Docs 對齊 + ObjectStorage::upload_bytes

狀態：done

## 目標

1. 將 architecture.md / api.md / deploy.md 同步到 SPEC-029~033 的實作現況
2. ObjectStorage trait 新增 `upload_bytes()` 方法，消除 thumbnail.rs 的 tempfile hack

## 約束

- **Docs 僅同步現有實作**：只修正與 SPEC-029~034 直接相關的漂移，不重寫整份文件語氣、結構或歷史段落，不處理與本次功能無關的舊描述瑕疵
- **upload_bytes() 只用於小型物件**（thumbnail / metadata blob），不取代大檔案的 `upload_file()` 流程。上傳前一次性持有完整 payload，不保證 streaming
- **不修改 API contract**：不改 thumbnail_url 格式、不改 DB schema、不改任何 endpoint 行為

## 背景

### Docs 漂移

SPEC-029~033 改動大：

| Spec | 影響 |
|------|------|
| SPEC-029 | bo-api split、common→error rename |
| SPEC-030 | Admin users（list / suspend / role） |
| SPEC-031 | Admin streams（list / detail / force-end / moderation） |
| SPEC-032 | Ban per-broadcaster |
| SPEC-033 | Rate limiting（rate-limit crate / middleware / env vars） |

三份 docs 都有未同步的內容。

### upload_bytes

`crates/api/src/handlers/thumbnail.rs` 用 `tempfile::tempdir()` 寫檔再讀回上傳，
對 in-memory 的 JPEG payload 來說多了不必要的磁碟 I/O。

## 影響範圍

### 修改

- `docs/architecture.md` — crate 清單、Redis key schema、middleware pipeline
- `docs/api.md` — admin endpoints、rate limit 表、TOC
- `docs/deploy.md` — bo-api port/compose/bootstrap/env vars、rate limit env vars
- `crates/storage/src/lib.rs` — ObjectStorage trait 加 `upload_bytes()`
- `crates/storage/src/lib.rs` — GcsStorage / MockStorage 實作 `upload_bytes()`
- `crates/api/src/handlers/thumbnail.rs` — 移除 tempfile，改用 `upload_bytes()`
- `crates/api/Cargo.toml` — 若無其他使用點，移除 `tempfile` 依賴

### 不改

- Entity / DB schema
- 前端
- API contract（endpoint path / request / response 格式）
- nginx config
- 任何 handler 邏輯（除 thumbnail 的上傳方式）
- 與 SPEC-029~034 無關的 docs 段落

## Todo list

- [x] SPEC-034-01 architecture.md — 更新 crate 清單（+bo-api, +rate-limit, common→error），更新元件邊界描述
- [x] SPEC-034-02 architecture.md — 補 Redis rate-limit key schema（`ratelimit:{bucket}:{id}`）+ middleware pipeline（unauthed→auth→authed→route-level）
- [x] SPEC-034-03 api.md — 補全 bo-api admin endpoints（SPEC-030~032 新增的 users / streams / moderation 端點，含 request/response 範例）
- [x] SPEC-034-04 api.md — rate limit 表補 bo-api general（60/min）+ chat（1/sec）行，修 TOC 編號
- [x] SPEC-034-05 deploy.md — 補 bo-api（port 表 8800 / bootstrap `cp deploy/bo/.env.example` / startup 順序 / compose 指令）
- [x] SPEC-034-06 deploy.md — 補 API rate limit env vars（16 個）+ bo-api env vars（含 rate limit 2 個）文件
- [x] SPEC-034-07 ObjectStorage trait 新增 `upload_bytes(&self, data: &[u8], key: &str) -> Result<String, StorageError>` 方法
- [x] SPEC-034-08 GcsStorage + MockStorage 實作 `upload_bytes()`
- [x] SPEC-034-09 thumbnail.rs 改用 `upload_bytes()`，移除 tempfile 依賴
- [x] SPEC-034-09b `crates/api/Cargo.toml` — 若已無使用點，移除 `tempfile` dependency
- [x] SPEC-034-10 測試：storage `upload_bytes` 單測 + thumbnail handler 回歸測試（驗證 DB update 行為不變、thumbnail_url 格式不變、不需 tempfile 也能成功）

## 驗收標準

- [x] architecture.md 列出目前 workspace 的所有 crate（含 bo-api、rate-limit、error）
- [x] architecture.md 有 rate limit Redis key schema 和 middleware pipeline 描述
- [x] api.md 文件涵蓋所有 bo-api admin endpoints（dashboard / users list / role update / suspend / unsuspend / streams / moderation / bans）
- [x] api.md rate limit 表有 bo-api general + chat 行
- [x] deploy.md 有 bo-api port 8800 + bootstrap + compose + env vars
- [x] deploy.md 有 18 個 rate limit env vars 文件
- [x] ObjectStorage trait 有 `upload_bytes` 方法
- [x] thumbnail handler 不再使用 tempfile
- [x] thumbnail_url 格式不變（`streams/{stream_key}/live-thumb.jpg`）
- [x] 所有既有測試通過 + 新增測試通過
- [x] CI 全綠（cargo build / test / clippy / fmt）

## 備註

- `upload_bytes` 簽名：`async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<String, StorageError>`。不用 `Bytes` 型別，`&[u8]` 最泛用
- docs/ 是 nested git repo（private streamhub-docs），docs 變更由 team lead 另行處理
- 此 spec 完成後，architecture.md / api.md / deploy.md 應與 SPEC-029~034 涉及的實作同步
