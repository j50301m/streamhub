# streamhub API 文件

> 最後更新：2026-04-09
> Base URL：`https://api.streamhub.com`
> API 版本：`v1`

---

## 目錄

1. [通用規範](#1-通用規範)
2. [認證](#2-認證)
3. [使用者](#3-使用者)
4. [串流管理](#4-串流管理)
5. [串流 Token](#5-串流-token)
6. [錄影](#6-錄影)
7. [Internal Hooks（MediaMTX）](#7-internal-hooksmediamtx)
8. [錯誤碼一覽](#8-錯誤碼一覽)

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

**列表：**
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

### 分頁參數

| 參數 | 型別 | 預設值 | 說明 |
|------|------|--------|------|
| `page` | integer | `1` | 頁碼（從 1 開始） |
| `per_page` | integer | `20` | 每頁筆數（最大 100） |

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

**Response `200 OK`：**

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

### 2.3 Refresh Token

```
POST /v1/auth/refresh
```

不需要認證。

**Request Body：**

```json
{
  "refresh_token": "dGhpcyBp..."
}
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
{
  "refresh_token": "dGhpcyBp..."
}
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
  "status": "Pending",
  "vod_status": "None",
  "urls": {
    "whip": "https://mediamtx.streamhub.com/7c9e6679.../whip",
    "whep": "https://mediamtx.streamhub.com/7c9e6679.../whep",
    "hls": "https://cdn.streamhub.com/7c9e6679.../index.m3u8",
    "embed": "https://streamhub.com/embed/7c9e6679..."  // planned,
    "vod_hls": null
  },
  "started_at": null,
  "ended_at": null,
  "created_at": "2026-01-01T00:00:00Z"
}
```

**status 狀態說明：**

| 值 | 說明 |
|----|------|
| `Pending` | 已建立，等待直播主推流 |
| `Live` | 直播進行中 |
| `Ended` | 直播結束，錄影處理中或完成 |
| `Error` | 異常中止 |

**vod_status 狀態說明：**

| 值 | 說明 |
|----|------|
| `None` | 尚未開始或無錄影 |
| `Processing` | Transcoder API 處理中 |
| `Ready` | VOD 可播放，`vod_hls` 有值 |
| `Failed` | 轉檔失敗 |

---

### 4.1 建立串流

```
POST /v1/streams
```

需要認證（role: `broadcaster`）。

**Request Body：**

```json
{
  "title": "我的第一場直播"
}
```

| 欄位 | 型別 | 必填 | 說明 |
|------|------|------|------|
| `title` | string | ✗ | 最多 200 字元 |

**Response `201 Created`：**

```json
{
  "data": {
    "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "user_id": "550e8400-e29b-41d4-a716-446655440000",
    "stream_key": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "title": "我的第一場直播",
    "status": "Pending",
    "vod_status": "None",
    "urls": {
      "whip": "https://mediamtx.streamhub.com/7c9e6679.../whip",
      "whep": "https://mediamtx.streamhub.com/7c9e6679.../whep",
      "hls": "https://cdn.streamhub.com/7c9e6679.../index.m3u8",
      "embed": "https://streamhub.com/embed/7c9e6679..."  // planned,
      "vod_hls": null
    },
    "started_at": null,
    "ended_at": null,
    "created_at": "2026-01-01T00:00:00Z"
  }
}
```

---

### 4.2 取得串流列表

```
GET /v1/streams
```

需要認證。只回傳當前使用者自己的串流。

**Query Parameters：**

| 參數 | 型別 | 說明 |
|------|------|------|
| `status` | string | 篩選狀態：`Pending` \| `Live` \| `Ended` \| `Error` |
| `vod_status` | string | 篩選 VOD 狀態：`None` \| `Processing` \| `Ready` \| `Failed` |
| `page` | integer | 頁碼 |
| `per_page` | integer | 每頁筆數 |

**Response `200 OK`：**

```json
{
  "data": [
    { ...Stream 物件... },
    { ...Stream 物件... }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 42,
    "total_pages": 3
  }
}
```

---

### 4.3 取得單一串流

```
GET /v1/streams/:id
```

不需要認證（公開端點，第三方網站可查詢）。

**Path Parameters：**

| 參數 | 說明 |
|------|------|
| `id` | Stream UUID 或 stream_key（兩者相同） |

**Response `200 OK`：**

```json
{
  "data": { ...Stream 物件... }
}
```

> **第三方用途**：用這個端點查詢 `urls.hls`、`urls.whep` 和 `status`，
> 確認串流在線後再嵌入播放器。

---

### 4.4 更新串流資訊

```
PATCH /v1/streams/:id
```

需要認證（必須是串流擁有者）。

**Request Body（所有欄位可選）：**

```json
{
  "title": "更新後的標題"
}
```

**Response `200 OK`：**

```json
{
  "data": { ...Stream 物件... }
}
```

---

### 4.5 刪除串流

```
DELETE /v1/streams/:id
```

需要認證（必須是串流擁有者）。
只有 `status = Pending` 或 `Ended` 的串流可以刪除，`Live` 狀態不可刪除。

**Response `204 No Content`**

---

### 4.6 結束直播

```
POST /v1/streams/:id/end
```

需要認證（必須是串流擁有者）。
強制結束一個 `Live` 狀態的串流（對應 MediaMTX 踢掉 publisher）。

**Response `200 OK`：**

```json
{
  "data": { ...Stream 物件，status: "Ended"... }
}
```

---

## 5. 串流 Token

直播主推流前需要取得一個短效 token，用於 MediaMTX 驗證推流身份。

### 5.1 取得推流 Token

```
POST /v1/streams/:id/token
```

需要認證（必須是串流擁有者，role: `broadcaster`）。

**Response `201 Created`：**

```json
{
  "data": {
    "token": "eyJzdHJlYW1f...",
    "expires_at": "2026-01-01T01:00:00Z",
    "whip_url": "https://mediamtx.streamhub.com/7c9e6679.../whip?token=eyJzdHJlYW1f..."
  }
}
```

Token 有效期 **1 小時**，使用一次後失效。

直播主用 `whip_url` 推流，MediaMTX 會在 `runOnPublish` 時向 API 驗證 token。

---

## 6. 錄影

### Recording 物件

```json
{
  "id": "a3bb189e-8bf9-3888-9912-ace4e6543002",
  "stream_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "gcs_path": "gs://streamhub-recordings-prod/7c9e6679.../2026-01-01_00-00-00.fmp4",
  "duration_secs": 3600,
  "file_size_bytes": 1073741824,
  "created_at": "2026-01-01T01:00:00Z"
}
```

---

### 6.1 取得串流的錄影列表

```
GET /v1/streams/:id/recordings
```

需要認證（必須是串流擁有者）。

**Response `200 OK`：**

```json
{
  "data": [
    { ...Recording 物件... }
  ],
  "pagination": { ... }
}
```

---

## 7. Internal Hooks（MediaMTX）

這些端點**只接受 GKE cluster 內部流量**（ClusterIP Service），不對外暴露，不需要 JWT 認證。

> ⚠️ 永遠不要在 Load Balancer 上開放這些路徑。

---

### 7.1 推流事件（Publish / Unpublish）

```
POST /internal/hooks/publish
```

**Request Body（由 MediaMTX `runOnPublish` / `runOnUnpublish` 觸發）：**

```json
{
  "stream_key": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "action": "publish"
}
```

| `action` 值 | 說明 |
|-------------|------|
| `publish` | 直播主開始推流，更新 `stream.status = Live`，記錄 `started_at` |
| `unpublish` | 直播主停止推流，更新 `stream.status = Ended`，記錄 `ended_at`，觸發 Transcoder |

**Response `200 OK`**

---

### 7.2 錄影段完成事件

```
POST /internal/hooks/recording
```

**Request Body（由 MediaMTX `runOnRecordSegmentComplete` 觸發）：**

```json
{
  "stream_key": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "file_path": "/recordings/7c9e6679.../2026-01-01_00-00-00.fmp4"
}
```

Rust hook handler 收到後：
1. 上傳 `file_path` 到 `gs://streamhub-recordings-{env}/`
2. 在 DB 建立 `recordings` 記錄
3. 刪除本地暫存檔

**Response `200 OK`**

---

### 7.3 MediaMTX 推流認證

```
POST /internal/auth
```

由 MediaMTX `auth.httpAddress` 設定觸發。MediaMTX 在每次有人推流或拉流時呼叫此端點驗證。

**Request Body（MediaMTX 格式）：**

```json
{
  "ip": "1.2.3.4",
  "user": "",
  "password": "",
  "path": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "protocol": "webrtc",
  "id": "abc123",
  "action": "publish",
  "query": "token=eyJzdHJlYW1f..."
}
```

| `action` 值 | 說明 |
|-------------|------|
| `publish` | 直播主推流，驗證推流 token（stream_tokens） |
| `read` | 觀眾拉流（WHEP / HLS），驗證串流是否存在且為 Live 狀態 |

**Response：**
- `200 OK`：認證通過，允許推流 / 拉流
- `401 Unauthorized`：token 無效或已過期，拒絕推流
- `404 Not Found`：串流不存在或非 Live 狀態（read action）

---

## 8. 錯誤碼一覽

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
| `409` | 衝突（例如刪除 Live 狀態的串流） |
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
| `STREAM_NOT_LIVE` | 409 | 操作要求串流為 Live 狀態 |
| `STREAM_ALREADY_LIVE` | 409 | 串流已在直播中 |
| `STREAM_CANNOT_DELETE` | 409 | Live 狀態的串流不可刪除 |
| `STREAM_TOKEN_EXPIRED` | 401 | 推流 token 已過期 |
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
      {
        "field": "email",
        "message": "must be a valid email address"
      },
      {
        "field": "password",
        "message": "must be at least 8 characters"
      }
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

// Step 3：取得推流 token
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

// 推流開始，stream.status 會變成 Live
```

---

## 附錄 B：第三方網站嵌入範例

```javascript
// 查詢串流狀態
const res = await fetch('https://api.streamhub.com/v1/streams/{stream_key}');
const { data: stream } = await res.json();

if (stream.status === 'Live') {
  // 用 hls.js 播放直播
  const video = document.getElementById('video');
  const hls = new Hls();
  hls.loadSource(stream.urls.hls);
  hls.attachMedia(video);
  video.play();
} else if (stream.vod_status === 'Ready') {
  // 播放 VOD
  const hls = new Hls();
  hls.loadSource(stream.urls.vod_hls);
  hls.attachMedia(video);
  video.play();
}
```
