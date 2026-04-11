# SPEC-002 認證 + 使用者管理

狀態：in-progress

## 目標

實作完整的 JWT 認證系統與使用者管理，讓串流操作綁定使用者身份，
並透過 MediaMTX HTTP auth 整合實現推流認證。

## 影響範圍

新增 / 修改：
- `crates/entity/src/user.rs` — users entity
- `crates/entity/src/stream_token.rs` — stream_tokens entity
- `crates/entity/src/stream.rs` — 加 user_id FK
- `crates/auth/` — JWT sign/verify、password hashing
- `crates/api/src/routes/auth.rs` — 認證 API endpoints
- `crates/api/src/routes/streams.rs` — 加 auth middleware、owner 權限
- `crates/api/src/middleware/` — Bearer token 驗證 middleware
- `deploy/services/mediamtx.yml` — 加 auth.httpAddress 設定

文件同步：
- [ ] docs/api.md（新增 auth endpoints 已在文件中，確認實作一致）

## Todo list

- [x] SPEC-002-01 entity — users entity（id, email, password_hash, role ActiveEnum: Broadcaster/Viewer/Admin, created_at）
- [x] SPEC-002-02 entity — stream_tokens entity（id, stream_id FK, token_hash, expires_at, created_at）
- [x] SPEC-002-03 entity — streams 加 user_id UUID FK 指向 users.id
- [x] SPEC-002-04 auth crate — password hashing（argon2）、JWT sign/verify（access token 24h, refresh token 30d）
- [x] SPEC-002-05 API — POST /v1/auth/register、POST /v1/auth/login（回傳 access_token + refresh_token）
- [x] SPEC-002-06 API — POST /v1/auth/refresh、POST /v1/auth/logout
- [x] SPEC-002-07 API — GET /v1/me（取得目前使用者資訊）
- [x] SPEC-002-08 Auth middleware — Bearer token 驗證，注入 current user 到 request extension，區分需認證 / 公開路由
- [x] SPEC-002-09 Stream 權限 — POST /v1/streams 需登入且綁定 user_id、PATCH/DELETE/end 只有 owner、GET 保持公開
- [x] SPEC-002-10 POST /v1/streams/:id/token — 產生短效推流 token（1hr），owner + broadcaster only
- [x] SPEC-002-11 POST /internal/auth — MediaMTX HTTP auth endpoint（publish 驗證 stream token、read 驗證 stream 是否 Live）
- [x] SPEC-002-12 deploy/services/mediamtx.yml — 加 auth type: http + httpAddress 指向 /internal/auth

## 驗收流程

### 前置準備

```bash
docker compose -f deploy/services/docker-compose.yml up -d
cargo run -p api
# schema 自動 sync（users, streams, stream_tokens 表都會建立）
```

### 認證流程

```bash
# 1. 註冊
curl -s -X POST localhost:8080/v1/auth/register \
  -H "Content-Type: application/json" \
  -d '{"email":"test@example.com","password":"test1234","role":"Broadcaster"}' | python3 -m json.tool
# 預期：201，回傳 user + access_token + refresh_token

# 2. 登入
curl -s -X POST localhost:8080/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"test@example.com","password":"test1234"}' | python3 -m json.tool
# 預期：200，回傳 access_token + refresh_token

# 3. 取得目前使用者
curl -s localhost:8080/v1/me \
  -H "Authorization: Bearer {access_token}" | python3 -m json.tool
# 預期：200，回傳 user 資訊

# 4. Refresh token
curl -s -X POST localhost:8080/v1/auth/refresh \
  -H "Content-Type: application/json" \
  -d '{"refresh_token":"{refresh_token}"}' | python3 -m json.tool
# 預期：200，回傳新 access_token + refresh_token
```

### 串流認證流程

```bash
# 5. 建立串流（需登入）
curl -s -X POST localhost:8080/v1/streams \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer {access_token}" \
  -d '{"title":"auth test"}' | python3 -m json.tool
# 預期：201，stream 有 user_id

# 6. 未登入建立串流
curl -s -X POST localhost:8080/v1/streams \
  -H "Content-Type: application/json" \
  -d '{"title":"no auth"}' | python3 -m json.tool
# 預期：401

# 7. 取得推流 token
curl -s -X POST localhost:8080/v1/streams/{stream_id}/token \
  -H "Authorization: Bearer {access_token}" | python3 -m json.tool
# 預期：201，回傳 token + whip_url（含 ?token=）

# 8. 公開查詢串流（不需登入）
curl -s localhost:8080/v1/streams/{stream_id} | python3 -m json.tool
# 預期：200
```

### 推流認證（瀏覽器測試）

1. 開啟 broadcaster 頁面，用帶 token 的 WHIP URL 推流 → 成功
2. 不帶 token 推流 → MediaMTX 拒絕（401）

### 觀看（不需登入）

1. 開啟 viewer 頁面，輸入 stream_key → WHEP/HLS 觀看成功（不需 token）

### 程式碼品質

```bash
cargo build && cargo test && cargo clippy -- -D warnings && cargo fmt --check
```

## 備註

- password hashing 用 argon2（不用 bcrypt，效能較好且是 OWASP 推薦）
- JWT secret 從環境變數 `JWT_SECRET` 讀取，config 加欄位
- refresh token 存 DB 或用 signed JWT（本輪用 signed JWT 簡化實作）
- stream_tokens 的 token 存 hash（不存明文），驗證時 hash 比對
- MediaMTX auth 對 read action 只檢查 stream 是否 Live，不要求 token（觀眾免登入）
