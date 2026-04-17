# streamhub API 文件

> 最後更新：2026-04-16
> Base URL：`https://api.streamhub.com`
> API 版本：`v1`

---

## 目錄

1. [通用規範](#1-通用規範)
2. [認證](#2-認證)
3. [使用者](#3-使用者)
4. [串流管理](#4-串流管理)
5. [串流 Token](#5-串流-token)
6. [縮圖](#6-縮圖)
7. [錄影 + Chat Moderation](#7-錄影)
8. [Admin API（bo-api）](#8-admin-apibo-apiport-8800)
9. [WebSocket 即時事件](#9-websocket-即時事件)
10. [Internal Hooks（MediaMTX / MTX drain / Transcoder）](#10-internal-hooks)
11. [錯誤碼一覽](#11-錯誤碼一覽)

---

## 1. 通用規範

### Base URL

```
https://api.streamhub.com/v1
```

### Request Headers

```http
Content-Type: application/json
Authorization: Bearer <jwt_token>   # 需要認證的端點
```

### Response 格式

所有回應統一包在 `data` 或 `error` 裡：

**成功：**
```json
{
  "data": { ... }
}
```

**列表（有分頁）：**
```json
{
  "data": [ ... ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 100,
    "total_pages": 5
  }
}
```

> 部分列表端點（例如 `/v1/streams/live`、`/v1/streams/vod`）回傳完整結果沒有分頁，只有 `data` 陣列。

**失敗：**
```json
{
  "error": {
    "code": "STREAM_NOT_FOUND",
    "message": "Stream not found",
    "details": null
  }
}
```

### 時間格式

所有時間欄位使用 ISO 8601 UTC：`2026-01-01T00:00:00Z`

### Enum 值（序列化慣例）

`stream.status` / `stream.vod_status` / `user.role` 等 ActiveEnum 欄位在 JSON 中**一律小寫**（`rename_all = "lowercase"`）。例如：`"pending"`, `"live"`, `"ended"`, `"error"`, `"none"`, `"processing"`, `"ready"`, `"failed"`, `"broadcaster"`, `"viewer"`, `"admin"`。Client 比對時請務必用小寫。

### 分頁參數

| 參數 | 型別 | 預設值 | 說明 |
|------|------|--------|------|
| `page` | integer | `1` | 頁碼（從 1 開始） |
| `per_page` | integer | `20` | 每頁筆數（最大 100） |

### URL 動態性

WHIP / WHEP / HLS URL 會由 API **根據目前選中的 MediaMTX 實例動態產生**，不是固定 host。
- 取得 WHIP URL：`POST /v1/streams/:id/token` 回應的 `whip_url`。
- 取得 WHEP / HLS URL：`GET /v1/streams/:id` 或 `/v1/streams/live` 回應裡的 `urls.whep` / `urls.hls`（只有 stream 狀態為 `live` 且有 active session 時會有值）。

### Rate Limiting

所有 API endpoint（`/internal/*` 除外）都有 rate limiting 保護。

#### Response Headers

所有正常回應都帶以下 headers：

```http
X-RateLimit-Limit: 120        # 此 policy 允許的最大請求數
X-RateLimit-Remaining: 15     # 此 window 剩餘請求數
X-RateLimit-Reset: 1713283200 # Window 重設時間（Unix epoch seconds）
```

#### 429 Too Many Requests

```json
{
  "error": {
    "code": "RATE_LIMITED",
    "message": "Too many requests, please try again later",
    "details": {
      "retry_after_seconds": 42
    }
  }
}
```

429 回應額外帶 `Retry-After` header（秒數）。

#### 限流規則

| Endpoint | Limit | Window | Key |
|----------|-------|--------|-----|
| `POST /v1/auth/register` | 5 | 15 min | IP |
| `POST /v1/auth/login` | 5 | 15 min | IP |
| `POST /v1/auth/refresh` | 10 | 1 min | user_id |
| `POST /v1/streams/:id/token` | 5 | 1 min | user_id |
| `GET /v1/ws` | 10 | 1 min | IP |
| Chat `send_chat`（WS） | 1 | 1 sec | user_id |
| General（已認證） | 120 | 1 min | user_id |
| General（未認證） | 30 | 1 min | IP |
| `/internal/*` | 不限流 | — | — |
| **bo-api** General | 60 | 1 min | user_id |

所有 limit / window 值皆可透過環境變數覆蓋，無需重新部署。

---

## 2. 認證

JWT token 有效期 24 小時。Refresh token 有效期 30 天。

### 2.1 註冊

```
POST /v1/auth/register
```

不需要認證。

**Request Body：**

```json
{
  "email": "jason@example.com",
  "password": "your_password",
  "role": "broadcaster"
}
```

| 欄位 | 型別 | 必填 | 說明 |
|------|------|------|------|
| `email` | string | ✓ | 唯一，小寫 |
| `password` | string | ✓ | 最少 8 字元 |
| `role` | string | ✓ | `broadcaster` \| `viewer` |

**Response `201 Created`：**

```json
{
  "data": {
    "user": {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "email": "jason@example.com",
      "role": "broadcaster",
      "created_at": "2026-01-01T00:00:00Z"
    },
    "access_token": "eyJhbGci...",
    "refresh_token": "dGhpcyBp...",
    "expires_in": 86400
  }
}
```

---

### 2.2 登入

```
POST /v1/auth/login
```

不需要認證。

**Request Body：**

```json
{
  "email": "jason@example.com",
  "password": "your_password"
}
```

**Response `200 OK`：** 同 2.1 的 `data` 結構。

---

### 2.3 Refresh Token

```
POST /v1/auth/refresh
```

不需要認證。

**Request Body：**

```json
{ "refresh_token": "dGhpcyBp..." }
```

**Response `200 OK`：**

```json
{
  "data": {
    "access_token": "eyJhbGci...",
    "refresh_token": "bmV3dG9r...",
    "expires_in": 86400
  }
}
```

---

### 2.4 登出

```
POST /v1/auth/logout
```

需要認證（Bearer token）。

**Request Body：**

```json
{ "refresh_token": "dGhpcyBp..." }
```

**Response `204 No Content`**

---

## 3. 使用者

### 3.1 取得目前使用者資訊

```
GET /v1/me
```

需要認證。

**Response `200 OK`：**

```json
{
  "data": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "email": "jason@example.com",
    "role": "broadcaster",
    "created_at": "2026-01-01T00:00:00Z"
  }
}
```

---

## 4. 串流管理

### Stream 物件

```json
{
  "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "stream_key": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "title": "我的第一場直播",
  "status": "pending",
  "vod_status": "none",
  "hls_url": null,
  "thumbnail_url": null,
  "urls": {
    "whip": null,
    "whep": null,
    "hls": null
  },
  "started_at": null,
  "ended_at": null,
  "created_at": "2026-01-01T00:00:00Z"
}
```

**欄位說明：**

| 欄位 | 說明 |
|------|------|
| `stream_key` | 即 `id` 的字串，MediaMTX path 以此為名 |
| `status` | `pending` / `live` / `ended` / `error` |
| `vod_status` | `none` / `processing` / `ready` / `failed` |
| `hls_url` | **VOD** HLS 播放 URL（`vod_status = ready` 後填入，指向物件儲存／CDN） |
| `thumbnail_url` | 縮圖 URL（由 live 抓圖 / VOD transcoder / 自訂上傳覆蓋） |
| `urls.whip` | **固定為 `null`**，broadcaster 應改向 `POST /:id/token` 取得臨時 WHIP URL |
| `urls.whep` | 即時 WEBRTC 播放 URL；只有當前 active session 存在時有值 |
| `urls.hls` | 即時 LL-HLS 播放 URL；只有 `live` 且有 active session 時有值 |
| `started_at` / `ended_at` | 直播 publish / unpublish 時戳 |

---

### 4.1 建立串流

```
POST /v1/streams
```

需要認證（role: `broadcaster` 或 `admin`）。

**Request Body：**

```json
{ "title": "我的第一場直播" }
```

| 欄位 | 型別 | 必填 | 說明 |
|------|------|------|------|
| `title` | string | ✗ | 最多 200 字元 |

**Response `201 Created`：**

```json
{ "data": { /* Stream 物件 */ } }
```

---

### 4.2 取得目前使用者的串流列表

```
GET /v1/streams
```

需要認證。只回傳當前使用者自己的串流。

**Query Parameters：**

| 參數 | 型別 | 說明 |
|------|------|------|
| `status` | string | 篩選：`Pending` \| `Live` \| `Ended` \| `Error`（**此 filter 目前接受 PascalCase**；回應仍為小寫） |
| `page` | integer | 頁碼 |
| `per_page` | integer | 每頁筆數（上限 100） |

**Response `200 OK`：**

```json
{
  "data": [ /* Stream 物件 */, ... ],
  "pagination": { "page": 1, "per_page": 20, "total": 42, "total_pages": 3 }
}
```

---

### 4.3 取得所有目前直播中的串流（公開）

```
GET /v1/streams/live
```

不需要認證。回傳所有 `status = live` 的串流（無分頁，精簡欄位給大廳頁用）。

**Response `200 OK`：**

```json
{
  "data": [
    {
      "id": "7c9e6679-...",
      "stream_key": "7c9e6679-...",
      "title": "...",
      "status": "live",
      "vod_status": "none",
      "hls_url": null,
      "thumbnail_url": "https://.../live-thumb.jpg",
      "started_at": "2026-04-14T10:00:00Z",
      "ended_at": null,
      "urls": {
        "whep": "http://localhost:8889/7c9e6679.../whep",
        "hls":  "http://localhost:8888/7c9e6679.../index.m3u8"
      }
    }
  ]
}
```

---

### 4.4 取得所有 VOD 串流（公開）

```
GET /v1/streams/vod
```

不需要認證。回傳 `vod_status = ready` 的串流（無分頁）。`hls_url` 指向 VOD HLS。

Response 結構同 §4.3。

---

### 4.5 取得單一串流

```
GET /v1/streams/:id
```

不需要認證（公開端點，第三方網站可查詢）。

**Path Parameters：**

| 參數 | 說明 |
|------|------|
| `id` | Stream UUID（與 `stream_key` 相同） |

**Response `200 OK`：**

```json
{ "data": { /* Stream 物件 */ } }
```

> **第三方用途**：用這個端點查 `urls.hls` / `urls.whep` 和 `status`，
> 確認串流在線後再嵌入播放器。**不要自己猜測 MediaMTX host**，URL 會依 API 實際路由而變。

---

### 4.6 更新串流資訊

```
PATCH /v1/streams/:id
```

需要認證（必須是串流擁有者）。

**Request Body（所有欄位可選）：**

```json
{ "title": "更新後的標題" }
```

**Response `200 OK`：** Stream 物件。

---

### 4.7 刪除串流

```
DELETE /v1/streams/:id
```

需要認證（必須是串流擁有者）。
只有非 `live` 的串流可以刪除；`live` 狀態會回 `409 STREAM_CANNOT_DELETE`。

**Response `204 No Content`**

---

### 4.8 結束直播

```
POST /v1/streams/:id/end
```

需要認證（必須是串流擁有者）。
只能用於 `status = live`；強制把 DB 狀態翻成 `ended`。（實際把 broadcaster 踢下線仍仰賴 MediaMTX 的 unpublish 事件 / 心跳。）

**Response `200 OK`：** Stream 物件（`status: "ended"`）。

---

## 5. 串流 Token

直播主推流前需要取得一個短效 token，用於 MediaMTX 驗證推流身份。
API 在發 token 的同時會決定這個 session 要落在哪一台 MediaMTX 實例上，並把 URL 直接填好回傳。

### 5.1 取得推流 Token + WHIP URL

```
POST /v1/streams/:id/token
```

需要認證（必須是串流擁有者，role: `broadcaster` 或 `admin`）。

**Response `201 Created`：**

```json
{
  "data": {
    "token": "eyJzdHJlYW1f...",
    "expires_at": "2026-01-01T01:00:00Z",
    "whip_url": "http://localhost:8889/7c9e6679.../whip?token=eyJzdHJlYW1f...&session=9a2c4e0a-..."
  }
}
```

| 欄位 | 說明 |
|------|------|
| `token` | Raw 推流 token，TTL 1 小時。也會夾在 `whip_url` 的 `?token=`。 |
| `expires_at` | Token 絕對過期時間（ISO 8601 UTC）。 |
| `whip_url` | 已經組好、可直接 `POST` SDP offer 的 WHIP endpoint，含 `?token=...&session=...` 兩個 query。Host 會是 API 當下為這個 session 選中的 MediaMTX 實例的 `public_whip`。 |

**運作細節：**

1. API 透過 Redis 中各 MediaMTX 實例的 `stream_count` + `status` 選出「最少負載且健康」的實例。
2. API 為本次推流建立一個新的 `session_id`，寫入 Redis（`session:{sid}:mtx`、`session:{sid}:stream_id`、`stream:{id}:active_session`）。
3. Raw token 經 SHA-256 後以 `stream_token:{hash} = stream_id` 寫入 Redis，**TTL 3600 秒**，Redis 到期自動清。**Token 在 TTL 內可重複使用**，不是 single-use。
4. 若 broadcaster 中途重連（或被 `drain` 通知），會再次呼叫本 endpoint 拿新的 token + 新 session + 新 MTX URL；舊 session 會被覆寫成 stale，對應的舊 webhook 到了也會被忽略。

直播主用 `whip_url` 推流；MediaMTX 在 `authMethod: http` 時會把 `token` query 原樣轉給 `/internal/auth` 驗證。

---

## 6. 縮圖

### 6.1 上傳自訂縮圖

```
POST /v1/streams/:id/thumbnail
```

需要認證（必須是串流擁有者）。

**Request Body：** 原始 JPEG 二進位（不是 JSON 包裝；`Content-Type` 為 `image/jpeg` 或任意 binary）。

**限制：**
- Body size ≤ 2 MiB
- Stream 必須為 `status = live` 或 `vod_status = ready`，其他狀態回 `409 STREAM_NOT_LIVE_OR_VOD_READY`。

**Response `200 OK`：**

```json
{
  "data": { "thumbnail_url": "https://.../streams/{stream_key}/live-thumb.jpg" }
}
```

上傳成功後 `stream.thumbnail_url` 會被覆寫為新的 URL。之後 live-thumbnail 週期性任務仍可能再覆蓋它（每 60s 抓一次）。

---

## 7. 錄影

### Recording 物件

```json
{
  "id": "a3bb189e-8bf9-3888-9912-ace4e6543002",
  "stream_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "file_path": "/recordings/7c9e6679.../2026-01-01_00-00-00.mp4",
  "duration_secs": null,
  "file_size_bytes": 1073741824,
  "created_at": "2026-01-01T01:00:00Z"
}
```

> `file_path` 是 MediaMTX 寫入的容器內路徑；實際上傳 GCS 與轉 HLS 在 unpublish 時觸發。

---

### 7.1 取得串流的錄影列表

```
GET /v1/streams/:id/recordings
```

需要認證。（同一 stream 上所有 recordings，分頁。）

**Query：** `page`, `per_page`。

**Response `200 OK`：**

```json
{
  "data": [ /* Recording 物件 */ ],
  "pagination": { "page": 1, "per_page": 20, "total": 5, "total_pages": 1 }
}
```

---

## 7.5 Chat Moderation

> Auth：broadcaster（自己的 stream）或 admin（任何 stream）。

### `DELETE /v1/streams/:stream_id/chat/messages/:msg_id`

刪除一則聊天訊息（hard delete）。

**Response**：`204 No Content`
**Errors**：`401` / `403`（非 owner 且非 admin）/ `404`（msg 不存在）

刪除後 server 透過 pub/sub 廣播 `chat_message_deleted` 事件給所有在該房間的 WS client。

### `POST /v1/streams/:stream_id/chat/bans`

禁言使用者。

**Body**：
```json
{ "user_id": "uuid", "duration_secs": 3600 }
```
`duration_secs` 可為 `600`（10min）、`3600`（1hr）、`86400`（1day）或 `null`（永久）。

**Response**：`201 Created`
```json
{ "user_id": "uuid", "expires_at": "2026-04-16T18:00:00Z" }
```
`expires_at` 為 `null` 時表示永久禁言。

**Errors**：`401` / `403` / `400`（duration 非法 / 自己 ban 自己）

### `GET /v1/streams/:stream_id/chat/bans`

列出該 stream 所有被禁言的使用者。

**Response**：`200 OK`
```json
{
  "data": [
    { "user_id": "uuid", "display_name": "foo", "expires_at": "2026-04-16T18:00:00Z" },
    { "user_id": "uuid", "display_name": "bar", "expires_at": null }
  ]
}
```

過期的 ban 在 list 時自動清理（lazy SREM），不回傳。

### `DELETE /v1/streams/:stream_id/chat/bans/:user_id`

解禁使用者。

**Response**：`204 No Content`
**Errors**：`401` / `403` / `404`（未被 ban）

---

## 8. Admin API（bo-api，port 8800）

> Base URL：`https://admin-api.streamhub.com`（本地：`http://localhost:8800`）
>
> 所有 bo-api 端點都需要 `AdminUser`（JWT role == admin）認證。使用同一組 `JWT_SECRET`。
>
> Rate limit：所有路由統一 60 req/min by user_id（`bo_general` policy）。

### 8.1 Dashboard

#### `GET /v1/admin/dashboard`

取得平台總覽數據。

**Response**：`200 OK`

```json
{
  "data": {
    "live_stream_count": 3,
    "total_user_count": 150,
    "broadcaster_count": 12,
    "ended_streams_24h": 8,
    "error_stream_count": 1,
    "recent_live_streams": [
      {
        "id": "uuid",
        "title": "Friday Night Stream",
        "stream_key": "uuid",
        "user_email": "broadcaster@example.com",
        "started_at": "2026-04-16T10:00:00Z",
        "viewer_count": 42
      }
    ]
  }
}
```

| Field | 說明 |
|---|---|
| `live_stream_count` | 目前 status=live 的串流數 |
| `total_user_count` | 總使用者數 |
| `broadcaster_count` | role=broadcaster 的使用者數 |
| `ended_streams_24h` | 過去 24 小時結束的串流數 |
| `error_stream_count` | status=error 的串流數 |
| `recent_live_streams` | 最近 10 筆 live stream + owner email + viewer count |

**Errors**：`401`（未認證）/ `403`（非 admin）

### 8.2 User Management

#### `GET /v1/admin/users`

列出使用者（含搜尋 / 篩選 / 分頁）。

**Query Parameters：**

| 參數 | 型別 | 預設 | 說明 |
|------|------|------|------|
| `page` | integer | `1` | 頁碼 |
| `per_page` | integer | `20` | 每頁筆數（上限 100） |
| `q` | string | — | email 模糊搜尋 |
| `role` | string | — | 篩選角色：`broadcaster` \| `viewer` \| `admin` |
| `suspended` | boolean | — | 篩選是否被停權 |

**Response `200 OK`：**

```json
{
  "data": {
    "users": [
      {
        "id": "uuid",
        "email": "user@example.com",
        "role": "broadcaster",
        "is_suspended": false,
        "suspended_until": null,
        "suspension_reason": null,
        "created_at": "2026-01-01T00:00:00Z"
      }
    ],
    "total": 150,
    "page": 1,
    "per_page": 20
  }
}
```

#### `PATCH /v1/admin/users/:id/role`

更新使用者角色。不能修改自己的角色。

**Request Body：**

```json
{ "role": "admin" }
```

| 欄位 | 型別 | 必填 | 說明 |
|------|------|------|------|
| `role` | string | ✓ | `broadcaster` \| `viewer` \| `admin` |

**Response `200 OK`：**

```json
{ "data": { /* UserResponse */ } }
```

**Errors**：`400`（修改自己）/ `404`（USER_NOT_FOUND）

#### `POST /v1/admin/users/:id/suspend`

停權使用者。不能停權自己。立即生效：寫 DB + Redis `user:state:{user_id}` = `"suspended"` + pub/sub 通知所有 API instance 斷開該使用者的 WS。

**Request Body：**

```json
{ "duration_secs": 3600, "reason": "spam" }
```

| 欄位 | 型別 | 必填 | 說明 |
|------|------|------|------|
| `duration_secs` | integer | ✗ | `null` = 永久；有值時 Redis key 設同長 TTL |
| `reason` | string | ✗ | 停權原因（存 DB） |

**Response `200 OK`：**

```json
{ "data": { /* UserResponse，is_suspended = true */ } }
```

**Errors**：`400`（停權自己）/ `404`（USER_NOT_FOUND）

#### `DELETE /v1/admin/users/:id/suspend`

解除停權（idempotent）。清除 DB suspended 欄位 + Redis 設 `user:state:{user_id}` = `"active"` EX 300。

**Response `200 OK`：**

```json
{ "data": { /* UserResponse，is_suspended = false */ } }
```

**Errors**：`404`（USER_NOT_FOUND）

### 8.3 Stream Management

#### `GET /v1/admin/streams`

列出所有串流（含搜尋 / 篩選 / 分頁）。

**Query Parameters：**

| 參數 | 型別 | 預設 | 說明 |
|------|------|------|------|
| `page` | integer | `1` | 頁碼 |
| `per_page` | integer | `20` | 每頁筆數（上限 100） |
| `status` | string | — | 篩選狀態：`pending` \| `live` \| `ended` \| `error` |
| `q` | string | — | title / stream_key 模糊搜尋 |

**Response `200 OK`：**

```json
{
  "data": {
    "streams": [
      {
        "id": "uuid",
        "title": "My Stream",
        "stream_key": "uuid",
        "status": "live",
        "vod_status": "none",
        "owner_email": "user@example.com",
        "started_at": "2026-04-16T10:00:00Z",
        "ended_at": null,
        "viewer_count": 42,
        "created_at": "2026-04-16T09:00:00Z"
      }
    ],
    "total": 50,
    "page": 1,
    "per_page": 20
  }
}
```

#### `GET /v1/admin/streams/:id`

串流詳細資訊（含 Redis enrichment：active_session / mtx_instance / chat_message_count）。

**Response `200 OK`：**

```json
{
  "data": {
    "id": "uuid",
    "title": "My Stream",
    "stream_key": "uuid",
    "status": "live",
    "vod_status": "none",
    "owner_id": "uuid",
    "owner_email": "user@example.com",
    "started_at": "2026-04-16T10:00:00Z",
    "ended_at": null,
    "hls_url": null,
    "thumbnail_url": "https://.../live-thumb.jpg",
    "viewer_count": 42,
    "created_at": "2026-04-16T09:00:00Z",
    "active_session": "session-uuid",
    "mtx_instance": "mtx-1",
    "chat_message_count": 128
  }
}
```

**Errors**：`404`（STREAM_NOT_FOUND）

#### `POST /v1/admin/streams/:id/end`

強制結束直播。將 DB status 設為 `ended`、設 `stream:{id}:force_ended` Redis flag 阻止 broadcaster 重連、pub/sub 通知 api 進行非同步清理（session keys / thumbnail task / MediaMTX kick）。

**Response `200 OK`：**

```json
{ "data": { /* StreamDetail */ } }
```

**Errors**：`404`（STREAM_NOT_FOUND）/ `409`（Stream is not live）

### 8.4 Moderation

#### `GET /v1/admin/moderation/bans`

列出所有 per-broadcaster 禁言紀錄（從最近 24h 有 stream 的 broadcaster 聚合 Redis bans）。

**Query Parameters：**

| 參數 | 型別 | 預設 | 說明 |
|------|------|------|------|
| `page` | integer | `1` | 頁碼 |
| `per_page` | integer | `20` | 每頁筆數（上限 100） |

**Response `200 OK`：**

```json
{
  "data": {
    "bans": [
      {
        "broadcaster_id": "uuid",
        "broadcaster_email": "streamer@example.com",
        "stream_id": "uuid",
        "user_id": "uuid",
        "user_email": "banned-user@example.com",
        "is_permanent": false
      }
    ],
    "total": 5,
    "page": 1,
    "per_page": 20
  }
}
```

> `stream_id` 是該 broadcaster 的代表 stream，admin 前端可用它呼叫 `DELETE /v1/streams/:stream_id/chat/bans/:user_id` 解禁。

#### `GET /v1/admin/moderation/streams/:id/chat`

檢視指定串流的聊天歷史（最近 100 則，newest first）。

**Response `200 OK`：**

```json
{
  "data": {
    "messages": [
      {
        "id": "msg-uuid-or-stream-entry-id",
        "user_id": "uuid",
        "display_name": "alice",
        "content": "hello",
        "ts": "1712345678901-0"
      }
    ]
  }
}
```

**Errors**：`404`（STREAM_NOT_FOUND）

### 8.5 Auth（bo-api）

bo-api 提供獨立的 login / refresh 端點，行為與 public API 相同但走 bo-api port：

- `POST /v1/auth/login` — 同 §2.2
- `POST /v1/auth/refresh` — 同 §2.3

---

## 9. WebSocket 即時事件

```
GET /v1/ws
```

WebSocket 升級 endpoint。不需要 `Authorization` header（公開），但可以帶 **optional** `?token=<access_token>` query 讓 server 辨識使用者 — 這是 **聊天發送** 的必要條件（未帶 token 的連線可以訂閱聊天和看歷史，但 `send_chat` 會回 `UNAUTHORIZED`）。

```
GET /v1/ws?token=<jwt_access_token>
```

建議 client 在首頁 / 觀看頁開啟，接收：

- 全站 live streams 列表更新（每次有人 publish/unpublish/first-thumbnail 都推）
- 指定 stream 的 viewer count
- 遇到 MediaMTX 被 drain / health-fail 時的 reconnect 通知
- 訂閱的聊天房間歷史與即時訊息

### 9.1 Server → Client 訊息

所有訊息都是 JSON，tag = `type`，欄位採 snake_case：

#### `live_streams`

Client 連線後會立刻收到一次初始快照，之後每次狀態變動都會再收到一次完整列表：

```json
{
  "type": "live_streams",
  "data": [
    {
      "id": "7c9e6679-...",
      "title": "我的直播",
      "stream_key": "7c9e6679-...",
      "status": "live",
      "thumbnail_url": "https://.../live-thumb.jpg",
      "started_at": "2026-04-14T10:00:00Z",
      "viewer_count": 42,
      "urls": {
        "whep": "http://localhost:8889/7c9e6679.../whep",
        "hls":  "http://localhost:8888/7c9e6679.../index.m3u8"
      }
    }
  ]
}
```

#### `viewer_count`

某個 stream 的觀眾人數更新：

```json
{ "type": "viewer_count", "stream_id": "7c9e6679-...", "count": 128 }
```

#### `reconnect`

API 要求 client 重連（MediaMTX 被 drain / health fail 時發）：

```json
{
  "type": "reconnect",
  "reason": "server_maintenance",
  "stream_ids": ["7c9e6679-...", "a1b2..."]
}
```

Broadcaster 收到時應重新呼叫 `POST /token` 拿新 URL，Viewer 收到時重新 `GET /streams/:id` 更新 `urls`。

#### `chat_history`

`subscribe_chat` 後立即推送，最近 50 則（oldest → newest）：

```json
{
  "type": "chat_history",
  "stream_id": "7c9e6679-...",
  "messages": [
    {
      "id": "1712345678901-0",
      "stream_id": "7c9e6679-...",
      "user_id": "...",
      "display_name": "alice",
      "content": "hello",
      "ts_ms": 1712345678901
    }
  ]
}
```

#### `chat_message`

即時訊息：

```json
{
  "type": "chat_message",
  "stream_id": "7c9e6679-...",
  "message": {
    "id": "1712345679000-0",
    "stream_id": "7c9e6679-...",
    "user_id": "...",
    "display_name": "alice",
    "content": "hi",
    "ts_ms": 1712345679000
  }
}
```

> ⚠️ `content` 是純文字，**前端渲染必須 HTML escape**（例如 DOM `textContent`），否則會 XSS。

#### `chat_message_deleted`

moderator / admin 刪除訊息後廣播給所有在該房間的 WS：

```json
{ "type": "chat_message_deleted", "stream_id": "7c9e6679-...", "msg_id": "01902a3b-4c5d-7e6f-..." }
```

前端收到後應從 DOM 移除對應 `msg_id` 的訊息元素。

#### `chat_error`

`send_chat` 被拒時回應：

```json
{ "type": "chat_error", "stream_id": "7c9e6679-...", "reason": "RATE_LIMITED" }
```

`reason` 可能值：

| reason | 情境 |
|---|---|
| `RATE_LIMITED` | 同一 user 1 秒內送超過 1 則 |
| `TOO_LONG` | 內容 > 500 字，或 trim 後為空字串 |
| `UNAUTHORIZED` | WS 連線沒帶 `?token=` JWT |
| `UNKNOWN_STREAM` | 該 `stream_id` 目前沒有 active session |
| `BANNED` | 使用者已被該 stream 禁言 |
| `UNKNOWN` | Redis 暫時失敗等內部錯誤 |

### 9.2 Client → Server 訊息

```json
{ "action": "subscribe",         "stream_id": "7c9e6679-..." }
{ "action": "unsubscribe",       "stream_id": "7c9e6679-..." }
{ "action": "subscribe_chat",    "stream_id": "7c9e6679-..." }
{ "action": "unsubscribe_chat",  "stream_id": "7c9e6679-..." }
{ "action": "send_chat",         "stream_id": "7c9e6679-...", "content": "hello" }
```

- `subscribe` / `unsubscribe`：關注 / 取消某 stream 的 `viewer_count` 推送。`live_streams` 推送不需要訂閱，所有 client 都會收到。
- `subscribe_chat`：加入聊天房間，server 立即回 `chat_history`，後續收到 `chat_message` 即時訊息。
- `unsubscribe_chat`：離開聊天房間。
- `send_chat`：送訊息。限制 — 必須已認證（WS 連線有 `?token=`）、content trim 後 1–500 字、同 user 每秒最多 1 則。違反時收到 `chat_error`，訊息不會被存也不會被廣播。

### 9.3 Multi-instance

多 API instance 透過 Redis Pub/Sub 同步：

- `streamhub:events`：`RedisEvent` JSON。publish webhook / drain / viewer_count 更新等發送；每個 instance 都訂閱並扇出。
- `streamhub:chat:{stream_id}`：聊天 JSON（`chat_message` 或 `chat_message_deleted` envelope）。`send_chat` 寫完 Redis Stream 後 `PUBLISH`；`delete_message` 在 XDEL 後 `PUBLISH`。每個 instance 在第一次本地 `subscribe_chat` 時才懶啟動該房間的 subscriber。**publisher instance 不做本地 fan-out**，避免訊息被雙重推送 — 所有 WS 一律透過 pub/sub loopback 收到。

---

## 10. Internal Hooks

這些端點**只接受 cluster / docker network 內部流量**（ClusterIP Service），不對外暴露，不需要 JWT 認證。

> ⚠️ 永遠不要在 Load Balancer 上開放 `/internal/*` 路徑。

---

### 10.1 推流事件（Publish / Unpublish）

```
POST /internal/hooks/publish?mtx={name}&query={mtx_query_string}
```

由 MediaMTX `runOnReady` / `runOnNotReady` 觸發。

**Query Parameters（非 body）：**

| 參數 | 說明 |
|------|------|
| `mtx` | 發出這個 webhook 的 MediaMTX 實例名（從 `entrypoint.sh` 把 `__MTX_NAME__` 替換進 yml）。缺省時 API 會 log warn 並 `200 OK` 忽略。 |
| `query` | MediaMTX 模板 `$MTX_QUERY` 的內容（含 `session=...`）。由於 axum Query 解析會把 `&` 當 top-level 參數分隔，實作上 `session` 也可能獨立出現為 top-level query，兩者皆支援。 |

**Request Body：**

```json
{
  "stream_key": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "action": "publish"
}
```

| `action` 值 | 說明 |
|-------------|------|
| `publish` | 驗證 `session_id` 與 Redis `stream:{id}:active_session` + `session:{sid}:mtx` 是否吻合；吻合才翻 `status=live`、`started_at=now`、`INCR mtx:{name}:stream_count`、spawn live thumbnail task、publish `live_streams` 事件。Session 不吻合 → 視為 stale，`200 OK` 忽略。 |
| `unpublish` | 若 session 是 active：翻 `status=ended`、`vod_status=processing`、`ended_at=now`、`end_session`（DECR + 清 Redis）、cancel thumbnail、spawn VOD transcode。若 session 是 stale：只清 Redis session keys + DECR 舊 MTX 計數，不動 DB。 |

**Response `200 OK`**（大部分情況回 200，即使 session stale 也回 200 讓 MediaMTX 不要重試）。
`500` 僅限 DB / Redis 錯誤。

---

### 10.2 錄影段完成事件

```
POST /internal/hooks/recording
```

**Request Body（由 MediaMTX `runOnRecordSegmentComplete` 觸發）：**

```json
{
  "stream_key": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "segment_path": "/recordings/7c9e6679.../2026-01-01_00-00-00.mp4"
}
```

Rust handler 收到後：

1. 把容器內的 `segment_path` 映射到 host 的 `recordings_path` 讀檔案大小。
2. 在 DB 建立 `recordings` 記錄（`file_path` 存原始 segment_path，`file_size_bytes` 若讀檔失敗則為 `null`）。
3. 實際上傳 GCS + 轉 HLS 發生在 `unpublish` 觸發的非同步 VOD task，這裡不處理媒體。

**Response `200 OK`**

---

### 10.3 MediaMTX 推流 / 拉流認證

```
POST /internal/auth
```

由 MediaMTX `authMethod: http` + `authHTTPAddress` 設定觸發。MediaMTX 在每次有人 publish / read 時 POST 這個 endpoint。

**Request Body（MediaMTX 原生格式）：**

```json
{
  "ip": "1.2.3.4",
  "user": "",
  "password": "",
  "path": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "protocol": "webrtc",
  "id": "abc123",
  "action": "publish",
  "query": "token=eyJzdHJlYW1f...&session=..."
}
```

| `action` 值 | 行為 |
|-------------|------|
| `publish` | 從 `query` 抽 `token=...`；SHA-256 hash 後 `GET stream_token:{hash}`；比對結果的 `stream_id` 是否 == `path`；吻合 → `200 OK`；token 遺失 / 無效 / 過期 → `401`；stream_id 不符 → `401`；stream 不存在 → `404`。 |
| `read` | 僅檢查 `stream_key = path` 的 stream 存在且 `status = live`；是 → `200`，否 → `404`。 |
| 其他（`api`, `playback` 等） | 預設放行 `200`。 |

**Response：**
- `200 OK` — 允許
- `401 Unauthorized` — token 無效 / 過期 / 不吻合
- `404 Not Found` — stream 不存在或非 Live（read）
- `500 Internal Server Error` — Redis / DB 故障

> 本 handler **不再查 PostgreSQL `stream_tokens` table**（SPEC-020 已 drop 該表）；token 全部查 Redis。

---

### 10.4 MediaMTX 實例 Drain

```
POST /internal/mtx/drain?mtx={name}
```

由 MediaMTX 容器 entrypoint.sh 在收到 `SIGTERM` 時呼叫，也可以人工 curl 手動 drain。

**Query Parameters：**

| 參數 | 說明 |
|------|------|
| `mtx` | 要標為 draining 的 MediaMTX 實例名（如 `mtx-2`） |

**行為：**

1. `SET mtx:{name}:status = "draining"`（此後 `select_instance` 不再選中它）。
2. 找出所有 `status=live` 且 `session:{sid}:mtx == {name}` 的 stream。
3. 透過 Redis `PUBLISH streamhub:events` 送 `reconnect { reason: "server_maintenance", stream_ids: [...] }`。
4. 各 API instance 的 WebSocket 把這個 event 扇出給所有 client。

**Response：** `200 OK`（即使沒有受影響的 stream）。

---

### 10.5 Transcoder 完成事件

```
POST /internal/hooks/transcoder-complete
```

由 GCP Pub/Sub push subscription 呼叫。Body 是 Pub/Sub envelope：

```json
{
  "message": {
    "data": "<base64-encoded Transcoder job event JSON>",
    "attributes": { ... }
  }
}
```

Base64 decode 後預期為 Transcoder job event，含 `job.state` 與 `job.labels.stream_id`（API `create_job` 時會把 `stream_id` 放進 labels）：

| `job.state` | 動作 |
|-------------|------|
| `SUCCEEDED` | `stream.vod_status = ready` |
| `FAILED` | `stream.vod_status = failed` |
| 其他（`PENDING` / `RUNNING` / …） | log debug，不動 DB |

生產建議改用 Pub/Sub OIDC 驗證取代 plain token；`PUBSUB_VERIFY_TOKEN` 目前只做 log。

**Response：** `200 OK`（讓 Pub/Sub 不要無限 retry），錯誤情況回 `400` / `500`。

---

## 11. 錯誤碼一覽

### HTTP Status Code

| Status | 說明 |
|--------|------|
| `200` | 成功 |
| `201` | 建立成功 |
| `204` | 成功，無回傳內容 |
| `400` | 請求格式錯誤或業務規則違反 |
| `401` | 未認證或 token 過期 |
| `403` | 已認證但無權限（例如存取別人的串流） |
| `404` | 資源不存在 |
| `409` | 衝突（例如刪除 Live 狀態的串流、縮圖上傳時狀態不符） |
| `422` | 欄位驗證失敗 |
| `500` | 伺服器內部錯誤 |

### 錯誤 `code` 列表

| `code` | HTTP Status | 說明 |
|--------|-------------|------|
| `INVALID_CREDENTIALS` | 401 | email 或密碼錯誤 |
| `TOKEN_EXPIRED` | 401 | JWT token 已過期 |
| `TOKEN_INVALID` | 401 | JWT token 格式無效 |
| `REFRESH_TOKEN_INVALID` | 401 | Refresh token 無效或已撤銷 |
| `FORBIDDEN` | 403 | 無權限存取此資源 |
| `USER_NOT_FOUND` | 404 | 使用者不存在 |
| `USER_ALREADY_EXISTS` | 409 | email 已被註冊 |
| `STREAM_NOT_FOUND` | 404 | 串流不存在 |
| `STREAM_NOT_LIVE` | 409 | 操作要求串流為 `live` 狀態 |
| `STREAM_NOT_LIVE_OR_VOD_READY` | 409 | 縮圖上傳要求 `live` 或 `ready` |
| `STREAM_ALREADY_LIVE` | 409 | 串流已在直播中 |
| `STREAM_CANNOT_DELETE` | 409 | `live` 狀態的串流不可刪除 |
| `STREAM_TOKEN_EXPIRED` | 401 | 推流 token 已過期（Redis 查不到） |
| `STREAM_TOKEN_INVALID` | 401 | 推流 token 無效 |
| `RECORDING_NOT_FOUND` | 404 | 錄影記錄不存在 |
| `VALIDATION_ERROR` | 422 | 欄位驗證失敗，`details` 含欄位錯誤列表 |
| `INTERNAL_ERROR` | 500 | 伺服器錯誤，請回報 |

**`VALIDATION_ERROR` 範例：**

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Validation failed",
    "details": [
      { "field": "email",    "message": "must be a valid email address" },
      { "field": "password", "message": "must be at least 8 characters" }
    ]
  }
}
```

---

## 附錄 A：直播主完整推流範例

```javascript
// Step 1：登入取得 access_token
const loginRes = await fetch('https://api.streamhub.com/v1/auth/login', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ email: 'jason@example.com', password: '...' })
});
const { data: { access_token } } = await loginRes.json();

// Step 2：建立串流
const streamRes = await fetch('https://api.streamhub.com/v1/streams', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Authorization': `Bearer ${access_token}`
  },
  body: JSON.stringify({ title: '我的直播' })
});
const { data: stream } = await streamRes.json();

// Step 3：取得推流 token（API 會選好 MediaMTX 實例並把 WHIP URL 填好）
const tokenRes = await fetch(`https://api.streamhub.com/v1/streams/${stream.id}/token`, {
  method: 'POST',
  headers: { 'Authorization': `Bearer ${access_token}` }
});
const { data: { whip_url } } = await tokenRes.json();

// Step 4：WebRTC 推流（WHIP）
const pc = new RTCPeerConnection({
  iceServers: [
    { urls: 'stun:stun.l.google.com:19302' },
    { urls: 'turn:turn.streamhub.com:3478', username: '...', credential: '...' }
  ]
});
const mediaStream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
mediaStream.getTracks().forEach(track => pc.addTrack(track, mediaStream));

const offer = await pc.createOffer();
await pc.setLocalDescription(offer);

// 等待 ICE gathering 完成
await new Promise(resolve => {
  if (pc.iceGatheringState === 'complete') { resolve(); return; }
  pc.addEventListener('icegatheringstatechange', () => {
    if (pc.iceGatheringState === 'complete') resolve();
  });
});

const whipRes = await fetch(whip_url, {
  method: 'POST',
  headers: { 'Content-Type': 'application/sdp' },
  body: pc.localDescription.sdp
});
const answerSdp = await whipRes.text();
await pc.setRemoteDescription({ type: 'answer', sdp: answerSdp });

// 推流開始；收到 WebSocket reconnect 時應重跑 Step 3
```

---

## 附錄 B：第三方網站嵌入範例

```javascript
// 查詢串流狀態
const res = await fetch('https://api.streamhub.com/v1/streams/{stream_key}');
const { data: stream } = await res.json();

if (stream.status === 'live' && stream.urls.hls) {
  // 用 hls.js 播放直播
  const video = document.getElementById('video');
  const hls = new Hls();
  hls.loadSource(stream.urls.hls);
  hls.attachMedia(video);
  video.play();
} else if (stream.vod_status === 'ready' && stream.hls_url) {
  // 播放 VOD
  const hls = new Hls();
  hls.loadSource(stream.hls_url);
  hls.attachMedia(video);
  video.play();
}
```

---

## 附錄 C：可選—WebSocket 訂閱大廳

```javascript
const ws = new WebSocket('wss://api.streamhub.com/v1/ws');
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  switch (msg.type) {
    case 'live_streams':
      renderLobby(msg.data);          // data: LiveStreamData[]
      break;
    case 'viewer_count':
      updateViewers(msg.stream_id, msg.count);
      break;
    case 'reconnect':
      if (msg.stream_ids.includes(currentStreamId)) reconnectPlayer();
      break;
  }
};

// 關注某個 stream 的 viewer 數
ws.send(JSON.stringify({ action: 'subscribe', stream_id: currentStreamId }));
```
