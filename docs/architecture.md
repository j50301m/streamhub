# streamhub — Architecture

> 最後更新：2026-04-09
> 對應 CLAUDE.md 版本：初版

---

## 目錄

1. [系統概覽](#1-系統概覽)
2. [元件說明](#2-元件說明)
3. [資料流](#3-資料流)
   - 3.1 直播主推流
   - 3.2 觀眾觀看（WebRTC 低延遲）
   - 3.3 觀眾觀看（HLS 手機/大規模）
   - 3.4 錄影與 VOD Pipeline
   - 3.5 第三方網站嵌入
4. [協定與端口一覽](#4-協定與端口一覽)
5. [Stream 生命週期狀態機](#5-stream-生命週期狀態機)
6. [資料庫 Schema](#6-資料庫-schema)
7. [GCP 基礎設施](#7-gcp-基礎設施)
8. [安全邊界](#8-安全邊界)
9. [Crate 依賴關係](#9-crate-依賴關係)

---

## 1. 系統概覽

```
┌─────────────────────────┐
│  Broadcaster (Browser)  │
│  Camera / Mic → WebRTC  │
└───────────┬─────────────┘
            │ WHIP
            ▼
┌───────────────────────────────────────────┐
│              MediaMTX                      │
│          (GKE — media router)              │
│                                            │
│  WHIP in ──► internal bus ──► WHEP out     │
│                           ├──► HLS out     │
│                           └──► record      │
└──┬──────────────┬──────────┬──────────────┘
   │ webhook       │ fMP4     │
   ▼               ▼          │
┌──────────┐  ┌─────────┐    │
│ Rust API │  │  GCS    │    │
│ (GKE)    │  │ Bucket  │    │
│          │  └────┬────┘    │
│ auth     │       │ trigger │
│ state    │  ┌────▼──────┐  │
│ hooks    │  │Transcoder │  │
└────┬─────┘  │ API (GCP) │  │
     │        └────┬──────┘  │
     ▼             │ HLS     │
┌─────────┐  ┌────▼──────┐  │
│Cloud SQL│  │ Cloud CDN │  │
│(PG 17)  │  └────┬──────┘  │
└─────────┘       │          │
                  ▼          ▼
          ┌────────────┐  ┌────────────┐
          │ Web Viewer │  │   Mobile   │
          │ WHEP / HLS │  │  HLS /App  │
          └────────────┘  └────────────┘

WebRTC ICE / NAT traversal:
  STUN: stun.l.google.com:19302
  TURN: coturn (GCE VM, static IP, asia-east1)
```

**核心設計原則：**

- **MediaMTX 是唯一的媒體平面（media plane）**：所有串流協定轉換、錄製、推送都在 MediaMTX 完成，Rust API server 不接觸任何媒體資料。
- **Rust API 是控制平面（control plane）**：負責認證、串流狀態管理、GCS 上傳觸發、對外 REST API。
- **兩者透過 HTTP webhook 解耦**：MediaMTX 串流事件 → POST → Rust hook handler。

---

## 2. 元件說明

### MediaMTX

| 屬性 | 值 |
|------|-----|
| 部署位置 | GKE Deployment，`streamhub-{env}` namespace |
| Image | `bluenviron/mediamtx:latest` |
| 協定 | WHIP in, WHEP out, LL-HLS out, RTSP out |
| 錄製 | fMP4，存到 PVC（後由 Rust hook 上傳 GCS） |
| API | REST on port `9997`（cluster 內部） |
| Path 規則 | 只接受符合 UUID v4 regex 的 stream key |

MediaMTX 的每個 **path** 對應一個直播間，path 名稱就是 stream key（UUID v4）。
同一個 path 的流可以同時被多個協定消費，協定轉換在 MediaMTX 內部完成。

### Rust API Server (`crates/api`)

| 屬性 | 值 |
|------|-----|
| Framework | Axum 0.8 |
| Port | `8080` |
| 對外 | GKE Service → GCP Load Balancer |
| 對內 | `/internal/hooks/*` 只接受 cluster 內流量 |

主要職責：
- JWT 認證（issue / validate token）
- 串流 CRUD 與狀態管理
- 接收 MediaMTX webhook 並更新 DB 狀態
- 接收 MediaMTX 錄影 webhook 並觸發 GCS 上傳
- 觸發 GCP Transcoder API（錄影完成後轉 multi-res HLS）
- 對第三方提供 stream 資訊 API（HLS URL、WHEP URL、狀態）

### crate 拆分

```
crates/
├── api/          # Axum router、handler、middleware
├── auth/         # JWT sign/verify，Workload Identity helper
├── stream/       # Stream 狀態機，業務邏輯
├── storage/      # GCS 上傳（google-cloud-storage crate 封裝）
├── transcoder/   # GCP Transcoder API 呼叫
├── hook/         # MediaMTX webhook payload 定義與處理
├── common/       # AppError, Config, DB pool 初始化
├── entity/       # SeaORM dense format entity 定義
└── migration/    # SeaORM migration（sea-orm-migration）
```

### Cloud SQL PostgreSQL 17

- 只有 GKE pod 透過 Workload Identity 連線，不對外暴露
- 連線使用 Cloud SQL Auth Proxy（sidecar 或 direct connector）
- `statement_timeout = 30s`（防止慢查詢）

### GCS Buckets

| Bucket | 用途 |
|--------|------|
| `streamhub-recordings-{env}` | MediaMTX 錄製的原始 fMP4（短期保留） |
| `streamhub-vod-{env}` | Transcoder API 輸出的 multi-res HLS（長期，CDN origin） |

### GCP Transcoder API

- 錄影完成後由 Rust `transcoder` crate 呼叫
- 輸入：`streamhub-recordings-{env}` 的 fMP4
- 輸出：`streamhub-vod-{env}` 的 HLS，包含 1080p / 720p / 480p
- 完成後透過 Pub/Sub 通知 Rust API 更新 VOD 狀態

### STUN / TURN

| 服務 | 位置 |
|------|------|
| STUN | `stun:stun.l.google.com:19302`（公開免費） |
| TURN | coturn on GCE VM，`e2-medium`，static external IP，`asia-east1-a` |

TURN 只在 WebRTC ICE 協商失敗（嚴格 NAT 環境）時使用，
大多數連線走 STUN 直連即可。

---

## 3. 資料流

### 3.1 直播主推流

```
Browser
  │
  ├─1─► GET /api/streams/:id/token          ← 取得推流 JWT
  │        Rust API 驗證身份，回傳 stream_key + JWT
  │
  ├─2─► WebRTC ICE negotiation
  │        STUN: stun.l.google.com
  │        TURN: coturn GCE (fallback)
  │
  ├─3─► POST https://mediamtx/{stream_key}/whip   ← WHIP 推流開始
  │        body: SDP offer
  │        MediaMTX 回傳 SDP answer
  │        媒體封包走 SRTP/DTLS over UDP
  │
  └─4─► MediaMTX 觸發 webhook
           POST http://api:8080/internal/hooks/publish
           body: { stream_key, action: "publish" }
           Rust API 更新 stream.status = Live
```

### 3.2 觀眾觀看（WebRTC 低延遲）

```
Web Viewer
  │
  ├─1─► GET /api/streams/:id                ← 取得 WHEP URL
  │
  ├─2─► WebRTC ICE negotiation (STUN/TURN)
  │
  └─3─► POST https://mediamtx/{stream_key}/whep    ← WHEP 拉流
           MediaMTX 直接從內部 bus 轉發媒體
           延遲 < 500ms
```

### 3.3 觀眾觀看（HLS 手機/大規模）

```
Mobile / Web Viewer
  │
  ├─1─► GET /api/streams/:id                ← 取得 HLS URL
  │
  └─2─► GET https://cdn.streamhub.com/{stream_key}/index.m3u8
           Cloud CDN → origin: MediaMTX HLS endpoint
           MediaMTX 即時產生 LL-HLS segments
           延遲 2~3s（LL-HLS）
```

即時直播時 CDN 回源至 MediaMTX HLS endpoint，但經 CDN 快取後大部分流量不需回源，適合大量並發觀眾。

### 3.4 錄影與 VOD Pipeline

```
MediaMTX
  │ 錄製中：每隔一段時間產生 fMP4 segment
  │
  ├─ 每個 segment 完成 ─►
  │     POST /internal/hooks/recording
  │     { stream_key, file_path: "/recordings/..." }
  │          │
  │          ▼
  │     Rust storage crate
  │     上傳 /recordings/*.fmp4 → GCS streamhub-recordings-{env}
  │
  └─ 直播結束（unpublish）─►
        POST /internal/hooks/publish { action: "unpublish" }
             │
             ▼
        Rust stream crate 更新 status = Ended
        Rust transcoder crate 呼叫 Transcoder API
             │  input:  gs://streamhub-recordings-{env}/{stream_key}/
             │  output: gs://streamhub-vod-{env}/{stream_key}/
             │  preset: 1080p + 720p + 480p HLS
             │
             ▼ (非同步，Transcoder API job)
        Pub/Sub 通知完成
             │
             ▼
        Rust API 更新 stream.vod_status = Ready
        VOD 可透過 Cloud CDN 播放：
        https://cdn.streamhub.com/vod/{stream_key}/index.m3u8
```

### 3.5 第三方網站嵌入

第三方只需要我們提供的以下任一 URL：

| 方式 | URL 格式 |
|------|---------|
| HLS（推薦） | `https://cdn.streamhub.com/{stream_key}/index.m3u8` |
| WHEP（低延遲） | `https://mediamtx.streamhub.com/{stream_key}/whep` |
| Embed iframe (planned) | `https://streamhub.com/embed/{stream_key}` |
| Status API | `GET https://api.streamhub.com/v1/streams/{stream_key}` |

CORS 已在 MediaMTX 和 API server 開放 `*`，第三方網頁可直接消費。

---

## 4. 協定與端口一覽

### 對外端口（Load Balancer / Cloud Armor）

| 服務 | 協定 | 端口 | 說明 |
|------|------|------|------|
| API | HTTPS | 443 | REST API，JWT 認證 |
| MediaMTX WHIP/WHEP | HTTPS | 443 | WebRTC signaling（HTTP） |
| MediaMTX WebRTC media | SRTP/UDP | 8189 | 媒體封包（ICE UDP mux，需在 mediamtx.yml 設定） |
| MediaMTX HLS | HTTPS | 443 | LL-HLS 播放（走 CDN） |
| TURN | TCP/UDP | 3478 | coturn GCE VM |
| TURN TLS | TCP | 5349 | coturn TLS（可選） |

### Cluster 內部端口

| 服務 | 端口 | 說明 |
|------|------|------|
| Rust API | 8080 | Axum HTTP |
| MediaMTX API | 9997 | REST 管理 API |
| MediaMTX RTSP | 8554 | 內部用（不對外） |
| Cloud SQL | 5433 | 透過 Auth Proxy（本地開發映射 5433:5432） |

---

## 5. Stream 生命週期狀態機

```
                    POST /streams
                         │
                         ▼
                    ┌─────────┐
                    │ Pending │  ← 已建立，等待直播主推流
                    └────┬────┘
                         │  MediaMTX webhook: publish
                         ▼
                    ┌─────────┐
                    │  Live   │  ← 正在直播
                    └────┬────┘
                         │  MediaMTX webhook: unpublish
                         ▼
                    ┌─────────┐
                    │  Ended  │  ← 直播結束，觸發錄影處理
                    └────┬────┘
                         │  Transcoder API 完成
                         ▼
                    ┌─────────┐
                    │  VodReady│  ← VOD 可播放
                    └─────────┘

                    （任何狀態可轉移到）
                    ┌──────────┐
                    │  Error   │  ← 異常中止
                    └──────────┘
```

狀態儲存在 `stream.status` 欄位（`ActiveEnum`）。
VOD 狀態另存在 `stream.vod_status`（`Processing` / `Ready` / `Failed`）。

---

## 6. 資料庫 Schema

### 主要 Tables

```
users
├── id              UUID PK
├── email           TEXT UNIQUE NOT NULL
├── password_hash   TEXT NOT NULL
├── role            ENUM (Broadcaster, Viewer, Admin)
└── created_at      TIMESTAMPTZ

streams
├── id              UUID PK
├── user_id         UUID FK → users.id
├── stream_key      TEXT UNIQUE NOT NULL     ← MediaMTX path
├── title           TEXT
├── status          ENUM (Pending, Live, Ended, Error)
├── vod_status      ENUM (None, Processing, Ready, Failed)
├── started_at      TIMESTAMPTZ
├── ended_at        TIMESTAMPTZ
├── hls_url         TEXT                     ← CDN HLS URL（VOD Ready 後填入）
└── created_at      TIMESTAMPTZ

recordings
├── id              UUID PK
├── stream_id       UUID FK → streams.id
├── gcs_path        TEXT NOT NULL            ← gs://bucket/path/file.fmp4
├── duration_secs   INTEGER
├── file_size_bytes BIGINT
└── created_at      TIMESTAMPTZ

stream_tokens
├── id              UUID PK
├── stream_id       UUID FK → streams.id
├── token_hash      TEXT NOT NULL
├── expires_at      TIMESTAMPTZ NOT NULL
└── created_at      TIMESTAMPTZ
```

### Entity 關聯

```
users ──── 1:N ──── streams ──── 1:N ──── recordings
                                 │
                                 └── 1:N ── stream_tokens
```

---

## 7. GCP 基礎設施

### Region

全部部署在 `asia-east1`（台灣）。

### GKE Cluster

```
streamhub-cluster (asia-east1)
├── node pool: default
│   └── e2-standard-4（2~6 nodes，依負載 autoscale）
│
└── namespaces:
    ├── streamhub-prod
    ├── streamhub-staging
    └── streamhub-dev
```

每個 namespace 包含：
- `Deployment/mediamtx`
- `Deployment/api`
- `Service/mediamtx`（LoadBalancer，對外）
- `Service/api`（LoadBalancer，對外）
- `Service/api-internal`（ClusterIP，給 MediaMTX webhook 用）
- `PersistentVolumeClaim/recordings`（MediaMTX 暫存錄影）

### 外部 VM

| VM | 規格 | 用途 |
|----|------|------|
| `coturn-prod` | e2-medium, `asia-east1-a` | TURN server，static IP |

### GCP 服務

| 服務 | 用途 |
|------|------|
| Cloud SQL (PostgreSQL 17) | 主資料庫，`asia-east1` |
| GCS `streamhub-recordings-{env}` | 原始 fMP4 暫存 |
| GCS `streamhub-vod-{env}` | 轉檔後 HLS，CDN origin |
| Cloud CDN | HLS 分發，指向 vod bucket |
| Transcoder API | 非同步多解析度轉檔 |
| Pub/Sub `transcoder-complete` | Transcoder 完成通知 |
| Workload Identity | GKE pod → GCP service 無 key 認證 |
| Cloud Armor | API / MediaMTX LoadBalancer 防護 |

---

## 8. 安全邊界

### 網路邊界

```
Internet
  │
  ├── Cloud Armor (WAF / DDoS)
  │     │
  │     ├── api.streamhub.com       → GKE Service/api (LoadBalancer)
  │     └── mediamtx.streamhub.com  → GKE Service/mediamtx (LoadBalancer)
  │
  └── cdn.streamhub.com             → Cloud CDN → GCS vod bucket

GKE internal (ClusterIP):
  mediamtx → POST http://api-internal:8080/internal/hooks/*
  api      → Cloud SQL Auth Proxy → Cloud SQL
  api      → GCS (Workload Identity)
  api      → Transcoder API (Workload Identity)
```

### 認證機制

| 路徑 | 認證方式 |
|------|---------|
| `POST /api/auth/login` | email + password → JWT |
| `POST /streams/:id/whip` | MediaMTX 透過 `runOnPublish` 向 Rust API 驗證 JWT |
| `/internal/hooks/*` | 只接受 ClusterIP，不經過 LoadBalancer |
| GCP services | Workload Identity（無 service account key） |
| Cloud SQL | Cloud SQL Auth Proxy + Workload Identity |

### MediaMTX 認證

```yaml
# mediamtx.yml
auth:
  type: http
  httpAddress: http://api-internal:8080/internal/auth
  # Rust API 收到請求後驗證 JWT query param
  # ?token=<jwt>
```

---

## 9. Crate 依賴關係

```
api
 ├── auth         (JWT)
 ├── stream       (狀態機)
 ├── hook         (webhook handler)
 ├── storage      (GCS)
 ├── transcoder   (Transcoder API)
 └── common       (AppError, Config, DB)

stream
 ├── entity       (SeaORM models)
 └── common

hook
 ├── stream
 └── common

storage
 └── common

transcoder
 └── common

entity             (SeaORM dense format entities)
 └── (no internal deps)

migration          (sea-orm-migration)
 └── entity
```

**原則：`common` 被所有人依賴，但 `common` 不依賴任何其他 internal crate。**
循環依賴一律不允許。
