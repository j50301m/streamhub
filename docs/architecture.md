# streamhub — Architecture

> 最後更新：2026-04-16
> 對應 CLAUDE.md 版本：SPEC-034 校準

---

## 目錄

1. [系統概覽](#1-系統概覽)
2. [元件說明](#2-元件說明)
3. [資料流](#3-資料流)
   - 3.1 直播主推流（含 session 機制）
   - 3.2 觀眾觀看（WebRTC 低延遲）
   - 3.3 觀眾觀看（HLS 手機/大規模）
   - 3.4 錄影與 VOD Pipeline
   - 3.5 縮圖 Pipeline（Live + VOD + 自訂）
   - 3.6 WebSocket 即時事件
   - 3.7 Graceful drain
   - 3.8 第三方網站嵌入
4. [協定與端口一覽](#4-協定與端口一覽)
5. [Stream 生命週期狀態機](#5-stream-生命週期狀態機)
6. [資料庫 Schema](#6-資料庫-schema)
7. [Redis 使用](#7-redis-使用)
8. [本地開發 Compose 切分](#8-本地開發-compose-切分)
9. [GCP 基礎設施](#9-gcp-基礎設施)
10. [安全邊界](#10-安全邊界)
11. [Crate 依賴關係](#11-crate-依賴關係)

---

## 1. 系統概覽

```
┌─────────────────────────┐
│  Broadcaster (Browser)  │
│  Camera / Mic → WebRTC  │
└───────────┬─────────────┘
            │ 1) POST /v1/streams/:id/token  ── API 選 mtx-X、產生 session
            │ 2) WHIP POST mtx-X/{key}/whip?token=...&session=...
            ▼
┌──────────────────────────────────────────────────┐
│  MediaMTX × N  (mtx-1 / mtx-2 / mtx-3 ...)        │
│         (媒體平面 — 每台獨立，不互通)              │
│                                                   │
│  WHIP in ──► internal bus ──► WHEP out            │
│                           ├──► LL-HLS out         │
│                           └──► record (fMP4)      │
└──┬──────────────┬──────────────┬─────────────────┘
   │ webhook       │ fMP4 on PVC │
   ▼               ▼             │
┌──────────┐  ┌─────────┐        │
│ Rust API │  │  GCS    │        │
│ (控制面) │  │ Bucket  │        │
│          │  └────┬────┘        │
│ auth     │       │ trigger     │
│ routing  │  ┌────▼──────┐      │
│ state    │  │Transcoder │      │
│ hooks    │  │ API (GCP) │      │
│ ws       │  └────┬──────┘      │
└──┬───┬───┘       │ HLS + thumb │
   │   │      ┌────▼──────┐      │
   │   │      │ Cloud CDN │      │
   │   │      └────┬──────┘      │
   ▼   ▼           │             ▼
┌──────┐ ┌────┐  ┌─▼──────────────┐
│ PG17 │ │Redis│ │ Web / Mobile   │
└──────┘ └────┘  │ Viewer (WHEP / │
                 │   HLS / WS)    │
                 └────────────────┘

WebRTC ICE / NAT traversal:
  STUN: stun.l.google.com:19302
  TURN: coturn (GCE VM, static IP, asia-east1)
```

**核心設計原則：**

- **MediaMTX 是唯一的媒體平面（media plane）**：所有串流協定轉換、錄製、推送都在 MediaMTX 完成，Rust API server 不接觸任何媒體資料。
- **Rust API 是控制平面（control plane）+ 路由大腦**：除了認證、狀態管理、GCS/Transcoder 觸發外，也負責把每個 stream 動態分派到某一台 MediaMTX 實例，並把實際的 WHIP/WHEP/HLS URL 回傳給 client。Client **不直接知道** MediaMTX 叢集的拓樸。
- **Redis 作為控制平面共享狀態**：stream → MTX 映射、session 配對、token、MTX 健康狀態、分散鎖、pub/sub 事件匯流排，全部住在 Redis（單一 instance，非叢集）。
- **多 API instance 透過 Redis Pub/Sub 同步**：WebSocket 推送、viewer_count 更新等事件由任一 API instance 寫入 Redis channel，所有 API instance 訂閱後廣播給自己的 WS 連線。

---

## 2. 元件說明

### MediaMTX × N（多實例）

| 屬性 | 值 |
|------|-----|
| 部署位置 | GKE Deployment（生產）／docker-compose `media` service（本地） |
| 預設實例數 | 本地開發 3 台（`mtx-1`, `mtx-2`, `mtx-3`），生產依負載橫向擴展 |
| Image | 自建（`Dockerfile.mediamtx`），基底 `bluenviron/mediamtx` |
| 協定 | WHIP in, WHEP out, LL-HLS out, RTSP out |
| 錄製 | fMP4，存到共享 `recordings` volume（本地）／PVC（GKE） |
| 管理 API | REST on port `9997`（cluster 內部，health check 用 `GET /v3/paths/list`） |
| Path 規則 | 只接受符合 UUID v4 regex 的 stream key |
| Webhook | `runOnReady` / `runOnNotReady` / `runOnRecordSegmentComplete` → Rust API（帶 `?mtx={name}&query=$MTX_QUERY`） |

每個 MediaMTX 實例有自己獨立的 WebRTC UDP port（例：mtx-1=8189, mtx-2=8199, mtx-3=8209），因為 Docker port mapping 對 ICE candidate 必須是 1:1 對應。

實例透過環境變數 `MTX_NAME` 自我識別，entrypoint.sh 在啟動時把 `mediamtx.yml` 模板裡的 `__MTX_NAME__` 和 `__WEBRTC_UDP_PORT__` 佔位符替換進去，webhook 觸發時就帶著自己的名字回 API。

### Rust API Server (`crates/api`)

| 屬性 | 值 |
|------|-----|
| Framework | Axum 0.8 |
| Port | `8080` |
| 對外 | 生產：GKE Service → Cloud Load Balancer；本地：nginx 反代 |
| 對內 | `/internal/hooks/*`、`/internal/auth`、`/internal/mtx/drain` 只接受 cluster / docker network 內流量 |

主要職責：

- JWT 認證（issue / validate token）
- 串流 CRUD 與狀態管理（state machine）
- **MediaMTX 路由決策**：`POST /token` 時依 Redis 中各 MTX 的 `stream_count` + `status` 選最少負載且健康的實例
- **Session 配對**：為每次推流生成 `session_id`，寫入 Redis，publish/unpublish webhook 必須帶回同一個 session_id 才算數
- 接收 MediaMTX webhook（publish / unpublish / 錄影段完成）
- 觸發 GCS 上傳 + GCP Transcoder API
- Transcoder 完成事件（Pub/Sub push）更新 VOD 狀態
- Live thumbnail 週期性擷取（每 60s）
- WebSocket `/v1/ws` 廣播 live streams、viewer count、reconnect 事件
- `/internal/mtx/drain` graceful drain endpoint

### Redis

| 屬性 | 值 |
|------|-----|
| 部署位置 | `infra` compose（本地 `redis:7-alpine`）／GKE StatefulSet（生產） |
| Port | 本地 host 6381 → container 6379 |
| 用途 | cache、session mapping、stream_token、MTX routing 狀態、distributed lock、pub/sub |

詳見 [§7 Redis 使用](#7-redis-使用)。

### crate 拆分

```
crates/
├── api/          # Axum router、handler、middleware、WebSocket（public API，port 8080）
├── bo-api/       # Admin backoffice API（獨立 binary，port 8800；dashboard / user / stream / moderation handlers）
├── auth/         # JWT sign/verify、stream token 產生/hash、Workload Identity helper
├── cache/        # Redis 封裝（CacheStore trait + InMemoryCache for test）
├── mediamtx/     # MTX routing / session 管理 / health check / Redis key schema
├── repo/         # UnitOfWork + Repository pattern（Stream/Recording/User repo）
├── storage/      # GCS / fake-gcs 上傳封裝（ObjectStorage trait：upload_file / upload_bytes / upload_dir）
├── transcoder/   # GCP Transcoder API + ffmpeg（local fallback、HLS thumbnail、MP4 concat）
├── rate-limit/   # Redis Lua script fixed window rate limiting（trait + middleware + IP extraction）
├── telemetry/    # 共用 OTel + Prometheus + JSON log 初始化、base HTTP metrics middleware
├── error/        # AppError 共用錯誤型別（原 common crate）
├── entity/       # SeaORM dense format entity 定義
└── migration/    # SeaORM migration
```

### Bo-API Server (`crates/bo-api`)

| 屬性 | 值 |
|------|-----|
| Framework | Axum 0.8 |
| Port | `8800` |
| 對外 | 生產：GKE Service → 獨立 domain / LoadBalancer；本地：nginx 反代 `/admin/api/` |
| 認證 | 共用同一組 `JWT_SECRET`（admin 前端用同一個 JWT 打 api 和 bo-api） |

主要職責：

- Admin dashboard（live/error/ended 統計 + recent live streams）
- 使用者管理（list / search / role update / suspend / unsuspend）
- 串流管理（list / detail / force-end）
- 聊天 moderation（bans 列表 / chat 歷史檢視）

bo-api 與 api 是獨立 binary，不同 Dockerfile / compose / .env，prod 部署在不同 domain。

**Observability**（與 api 同步於 SPEC-035）：

- OpenTelemetry OTLP/gRPC traces → Tempo（`OTEL_EXPORTER_OTLP_ENDPOINT`，service name `streamhub-bo-api`）
- Prometheus metrics：`GET /metrics`，無 JWT / rate-limit（Prometheus scrape endpoint）。base counter/histogram 由 `telemetry::base_http_metrics` 中介軟體產生
- JSON logs 含 OTel `trace_id` 欄位 → Loki，可從 log 一鍵跳到 Tempo 看完整流程
- 所有 admin handler 以及 `force_end` / `suspend` / `list_bans` 的關鍵子流程都加上 `#[tracing::instrument]`

**Trace Context Propagation**（SPEC-036）：

bo-api 發起的 admin 動作（`force_end` / `suspend`）會透過 Redis pubsub 與 api 溝通；`telemetry` crate 在 `init_telemetry` 設定 W3C `TraceContextPropagator`，publisher / subscriber 透過 payload 內的 `traceparent` 欄位串接 trace，讓 Tempo 看到一條連續的跨服務 trace。

- **Redis pubsub carrier**：`admin_force_end` / `user_suspended` / `streamhub:events`（`RedisEvent` enum）/ `streamhub:chat:{id}`（`TracedServerMessage` envelope）payload 都含 `traceparent: Option<String>`；publisher 呼叫 `telemetry::inject_traceparent()` 注入，subscriber 呼叫 `telemetry::set_parent_from_traceparent(&span, ...)` 恢復 parent context。向後相容：舊 payload 沒有 `traceparent` 欄位或格式錯誤時，subscriber 建新 root span 不 panic
- **Chat 不污染 client schema**：`TracedServerMessage` 是 pubsub-only envelope，subscriber 把內層 `ServerMessage` 單獨 forward 給 WS client，`traceparent` 僅停留在伺服器內部
- **HTTP inbound extractor**：api + bo-api 的 `TraceLayer` 用 `telemetry::http_make_span`，從 incoming `traceparent` header 繼承 parent context（MediaMTX 目前不送，但 frontend 或其他服務若送會自動接上）
- **不包含**：outbound HTTP propagation（api → MediaMTX / GCS / Transcoder）留給未來 spec；需要 `reqwest-middleware` 依賴，收益有限

### Cloud SQL PostgreSQL 17

- 只有 GKE pod 透過 Workload Identity 連線，不對外暴露
- 連線使用 Cloud SQL Auth Proxy（sidecar 或 direct connector）
- `statement_timeout = 30s`（防止慢查詢）
- 本地開發：`infra` compose 裡的 `postgres:17`，host 5433 → container 5432

### GCS Buckets

| Bucket | 用途 |
|--------|------|
| `streamhub-recordings-{env}` | MediaMTX 錄製的原始 fMP4（短期保留） |
| `streamhub-vod-{env}` | Transcoder API 輸出的 multi-res HLS（長期，CDN origin） |

本地開發用 fake-gcs-server 模擬（`infra` compose，port 4443）。

### GCP Transcoder API

- 錄影完成（unpublish webhook）後由 Rust `publish` handler spawn 非同步 task 呼叫
- 輸入：上傳到 GCS 的 `input.mp4`（先用 ffmpeg 把多個 fMP4 segment concat）
- 輸出：`streamhub-vod-{env}` 的 HLS，包含 1080p / 720p / 480p
- 額外產出 `spriteSheets` 作為 VOD 縮圖
- 完成後透過 Pub/Sub push 到 `/internal/hooks/transcoder-complete` 更新 `vod_status`

### STUN / TURN

| 服務 | 位置 |
|------|------|
| STUN | `stun:stun.l.google.com:19302`（公開免費） |
| TURN | coturn on GCE VM，`e2-medium`，static external IP，`asia-east1-a` |

TURN 只在 WebRTC ICE 協商失敗（嚴格 NAT 環境）時使用，
大多數連線走 STUN 直連即可。

---

## 3. 資料流

### 3.1 直播主推流（含 session 機制）

```
Browser                                API                           Redis                        mtx-X
  │                                     │                              │                            │
  ├─1─► POST /v1/streams/:id/token ─────►│                              │                            │
  │                                     │ select_instance(健康+最少流量)                             │
  │                                     ├──GET mtx:{n}:status / stream_count──────►│                 │
  │                                     │◄────────────────────────────────────────┤                 │
  │                                     │ create_session(stream_id, mtx-X)                           │
  │                                     ├──SET session:{sid}:mtx / stream_id / started_at──►        │
  │                                     ├──SET stream:{id}:active_session = sid────►                │
  │                                     │ SET stream_token:{hash(token)} = stream_id EX 3600         │
  │◄──── { token, whip_url, expires_at }─┤                              │                            │
  │         whip_url 格式：                                                                         │
  │         {mtx-X public_whip}/{stream_key}/whip?token=...&session=...                             │
  │                                     │                              │                            │
  ├─2─► WebRTC ICE negotiation (STUN / TURN fallback)                                                │
  │                                                                                                  │
  ├─3─► POST {whip_url}  (body: SDP offer) ────────────────────────────────────────────────────────►│
  │                                     │                              │              (mtx-X 要求認證)
  │                                     │◄── POST /internal/auth ──────────────────────────────────┤
  │                                     │   { path, action=publish, query=token=... }               │
  │                                     │ hash(token) 查 Redis stream_token:{hash}                  │
  │                                     ├──GET stream_token:{hash} ──►│                            │
  │                                     │◄─ stream_id ────────────────┤                            │
  │                                     │ 比對 path == stream_id → 200 OK                           │
  │                                     ├──────────────────────────────────────────────────────────►│
  │◄──── SDP answer ────────────────────────────────────────────────────────────────────────────────┤
  │                                                                                                  │
  │            媒體封包 SRTP/DTLS over UDP (mtx-X 的 WebRTC UDP port) ◄─────────────────────────────┤
  │                                                                                                  │
  │                                     │◄── runOnReady webhook ───────────────────────────────────┤
  │                                     │   POST /internal/hooks/publish?mtx=mtx-X&query=session=...│
  │                                     │   body: { stream_key, action: "publish" }                 │
  │                                     │ 比對 stream:{id}:active_session == session_id               │
  │                                     │ 比對 session:{sid}:mtx == mtx-X                           │
  │                                     │ 若符：stream.status = Live, started_at = now              │
  │                                     │       INCR mtx:{mtx-X}:stream_count                       │
  │                                     │       spawn live-thumbnail 任務                           │
  │                                     │       publish Redis "streamhub:events" live_streams       │
  │                                     │ 若不符（stale session）：log warn + 忽略                  │
```

**關鍵：session 配對為什麼重要？**

當某台 MTX 掛掉 / 被 drain 時，API 會通知 broadcaster 重連。Broadcaster 重連會拿新 token + 新 session_id，於是 `stream:{id}:active_session` 被覆寫為新的 sid。舊 MTX 如果之後送出任何 publish/unpublish webhook，API 會發現 session 不符就直接忽略，避免舊的 webhook 把新的 Live session 意外推到 Ended。

### 3.2 觀眾觀看（WebRTC 低延遲）

```
Web Viewer
  │
  ├─1─► GET /v1/streams/:id   (或 /v1/streams/live)  ◄── API 根據 stream 的 active_session 回傳對應 mtx-X 的 URL
  │        回應含 urls.whep = "{mtx-X public_whep}/{stream_key}/whep"
  │
  ├─2─► WebRTC ICE negotiation (STUN/TURN)
  │
  └─3─► POST {urls.whep}   ── mtx-X 直接從內部 bus 轉發媒體（延遲 < 500ms）
          MediaMTX 同樣對每筆 read 觸發 /internal/auth；action=read 僅檢查 stream 存在且 status=Live
```

### 3.3 觀眾觀看（HLS 手機/大規模）

```
Mobile / Web Viewer
  │
  ├─1─► GET /v1/streams/:id   ◄── urls.hls = "{mtx-X public_hls}/{stream_key}/index.m3u8"
  │                            （生產建議放 Cloud CDN 前置快取，CDN origin 指到 mtx 的 HLS endpoint）
  │
  └─2─► GET {urls.hls}
           MediaMTX 即時產生 LL-HLS segments，延遲 2~3s
```

因為 URL 是動態的，第三方務必使用 API 回傳的 `urls`，不要自己拼 host。

### 3.4 錄影與 VOD Pipeline

```
mtx-X
  │ 錄製中：每隔一段時間產生 fMP4 segment（寫到共享 recordings volume）
  │
  ├─ 每段完成 ─►
  │     POST /internal/hooks/recording
  │     { stream_key, segment_path: "/recordings/.../*.mp4" }
  │          │
  │          ▼
  │     Rust：在 DB 建立 recordings 列，記錄 file_path + file_size_bytes
  │
  └─ 直播結束 ─► runOnNotReady ─►
        POST /internal/hooks/publish?mtx=mtx-X&query=session=... { action: "unpublish" }
             │
             ▼
        比對 session；若是 active session：
          stream.status = Ended, ended_at = now, vod_status = Processing
          DECR mtx:{mtx-X}:stream_count；清掉 session:{sid}:* + stream:{id}:active_session
          cancel live-thumbnail 任務
          publish live_streams 事件
          spawn 非同步 VOD task：
            1. 用 ffmpeg concat 該 stream 目錄下的所有 fMP4 為 combined.mp4
            2. 走 GCP 路徑（有 transcoder_project_id）：
                 - 上傳 combined.mp4 → gs://streamhub-recordings-{env}/streams/{key}/input.mp4
                 - transcoder.create_job(...) 建立 multi-res HLS + spriteSheets 作業
                 - 寫入預期的 hls_url / thumbnail_url
                 - Transcoder 完成 → Pub/Sub push → /internal/hooks/transcoder-complete → vod_status=Ready|Failed
               走 local 路徑（未設 transcoder_project_id）：
                 - ffmpeg 直接轉 HLS，上傳到 object storage，寫 hls_url / thumbnail_url / vod_status=Ready
```

### 3.5 縮圖 Pipeline（Live + VOD + 自訂）

縮圖由三條獨立路徑共用同一個 `stream.thumbnail_url` 欄位，後進者覆蓋前者：

1. **Live thumbnail（每 60s）**
   publish webhook 確認是 active session 後，API spawn 一個 tokio task，每 60s 從 MTX 自己的 HLS endpoint（`http://mtx-X:8888/{stream_key}/index.m3u8`）用 ffmpeg 抓一張 `live-thumb.jpg`，上傳 object storage，更新 DB。unpublish 時 cancel。
2. **VOD thumbnail**
   - 走 GCP Transcoder：透過 `spriteSheets` preset 產出 `thumb0000000000.jpeg`。
   - 走 local ffmpeg：直接 extract first frame 並上傳。
3. **自訂縮圖上傳（owner only）**
   `POST /v1/streams/:id/thumbnail`（body = raw JPEG bytes，限制 2 MiB）。只有 Live 或 VodReady 狀態可上傳。上傳成功後同樣覆蓋 `thumbnail_url`。

### 3.6 WebSocket 即時事件

```
                     ┌───────────────────────── Redis PubSub "streamhub:events" ──────────────────────────┐
                     │                                                                                     │
 publish hook        │                                                                                     │
 unpublish hook      │                                                                                     │
 drain handler       │                                                                                     │
 thumbnail 首次產出  │                                                                                     │
                     ▼                                                                                     │
   任一 API instance PUBLISH RedisEvent JSON                                                               │
                                                                                                           │
                          每個 API instance subscribe "streamhub:events" ◄────────────────────────────────┘
                                  │
                                  ▼
                           WsManager 扇出給本 instance 的所有 WS 連線
                                  │
                   ┌──────────────┼──────────────┐
                   ▼              ▼              ▼
              Browser A       Browser B       Browser C   (各 instance 獨立持有自己的 WS 連線集合)
```

訊息 schema（`type` tag + snake_case）：

| 類型 | 方向 | 說明 |
|------|------|------|
| `live_streams` | S→C | 完整 live 列表快照（connect 時 + 每次 publish/unpublish/first-thumbnail 都推一次） |
| `viewer_count` | S→C | 單一 stream 的觀眾人數更新（`{ stream_id, count }`） |
| `reconnect` | S→C | 指定 stream_ids 的 client 應該重連（`{ reason, stream_ids }`） |
| `subscribe` / `unsubscribe` | C→S | client 表示關注 / 取消關注某 stream 的 viewer_count |

Viewer count 實作：WsManager 用 `viewer_count_lock`（Redis 分散鎖）確保多 instance 下每個 stream 的計數不會重複統計。

### 3.7 Graceful drain

用於滾動升級 / 手動下線 MediaMTX 實例：

```
SIGTERM → mtx-X 的 entrypoint.sh 攔截
          ├─ curl POST http://api:8080/internal/mtx/drain?mtx=mtx-X
          │      API：
          │        1) Redis SET mtx:{mtx-X}:status = "draining"（往後 select_instance 不會再選它）
          │        2) 找出所有 active session 在 mtx-X 上的 live stream
          │        3) Redis PUBLISH reconnect event { reason: "server_maintenance", stream_ids: [...] }
          │             → WebSocket 客戶端收到 → Broadcaster UI 自動重連
          │             → 重連會打 POST /token，API 選新的 MTX，寫新 session
          ├─ sleep 5（給 client 時間切走）
          └─ kill -TERM MTX_PID（MediaMTX 自己收尾）
```

Health check（每 10s 跑一輪）連續 3 次失敗會把 `mtx:{name}:status` 設為 `unhealthy`，效果跟 drain 類似：不再被選中、發 reconnect event。

### 3.8 第三方網站嵌入

第三方先 `GET /v1/streams/:id`（或訂閱 `/v1/ws` 的 `live_streams`）拿到 URL：

| 方式 | URL 來源 |
|------|---------|
| HLS（推薦） | 回應中的 `urls.hls`（CDN 包過後更佳） |
| WHEP（低延遲） | 回應中的 `urls.whep` |
| Embed iframe (planned) | `https://streamhub.com/embed/{stream_key}` |
| Status API | `GET /v1/streams/{id}` |

CORS 已在 MediaMTX 和 API 開放 `*`，第三方網頁可直接消費。

---

## 4. 協定與端口一覽

### 對外端口（Load Balancer / Cloud Armor）

| 服務 | 協定 | 端口 | 說明 |
|------|------|------|------|
| API | HTTPS | 443 | REST API + WebSocket，JWT 認證 |
| MediaMTX WHIP/WHEP | HTTPS | 443 | WebRTC signaling（HTTP） |
| MediaMTX WebRTC media | SRTP/UDP | 每實例獨立（例：8189/8199/8209） | 媒體封包（ICE UDP mux），每實例需 host port 1:1 對應 |
| MediaMTX HLS | HTTPS | 443 | LL-HLS 播放（建議前置 CDN） |
| TURN | TCP/UDP | 3478 | coturn GCE VM |
| TURN TLS | TCP | 5349 | coturn TLS（可選） |

### 本地開發（docker-compose）對外 port

| 服務 | Host port → container |
|------|-----------------------|
| nginx（反代 API + Web） | 3000 → 3000 |
| API（直連除錯用） | 8080 → 8080 |
| mtx-1 HLS / WebRTC / RTSP | 8888/8889/8554 |
| mtx-2 HLS / WebRTC / RTSP | 8898/8899/8564 |
| mtx-3 HLS / WebRTC / RTSP | 8908/8909/8574 |
| mtx-1/2/3 WebRTC UDP | 8189/8199/8209 |
| PostgreSQL | 5433 → 5432 |
| Redis | 6381 → 6379 |
| fake-gcs | 4443 → 4443 |

### Cluster 內部端口

| 服務 | 端口 | 說明 |
|------|------|------|
| Rust API | 8080 | Axum HTTP + WebSocket |
| MediaMTX API | 9997 | REST 管理 API，健康檢查 `GET /v3/paths/list` |
| MediaMTX HLS | 8888 | 內部 live thumbnail 任務從這裡抓 |
| MediaMTX WebRTC | 8889 | 內部 WHIP/WHEP |
| MediaMTX RTSP | 8554 | 內部用（不對外） |
| Cloud SQL | 5432 | 透過 Auth Proxy |

---

## 5. Stream 生命週期狀態機

```
                    POST /v1/streams
                         │
                         ▼
                    ┌─────────┐
                    │ pending │  ← 已建立，等待直播主推流
                    └────┬────┘
                         │  publish webhook（active session 配對成功）
                         ▼
                    ┌─────────┐
                    │  live   │  ← 正在直播
                    └────┬────┘
                         │  unpublish webhook（active session）/ POST /:id/end
                         ▼
                    ┌─────────┐
                    │  ended  │  ← 直播結束，vod_status 進入 processing
                    └────┬────┘
                         │  Transcoder 完成 (Pub/Sub) / local ffmpeg 完成
                         ▼
                    ┌─────────┐
                    │ VodReady│  ← vod_status=ready，可播 VOD HLS
                    └─────────┘

                    （任何狀態可轉移到）
                    ┌──────────┐
                    │  error   │  ← 異常中止
                    └──────────┘
```

狀態儲存在 `stream.status`（`StreamStatus` ActiveEnum）。
VOD 狀態獨立在 `stream.vod_status`（`none` / `processing` / `ready` / `failed`）。
ActiveEnum 在 JSON 序列化時是小寫（`rename_all = "lowercase"`）。

**Session vs. Status**：一個 stream 可以經歷多個 session（broadcaster 斷線重連、MTX drain 重分配）。只有目前 `stream:{id}:active_session` 所指向的那一個 session 的 webhook 會改動 DB 狀態，其他都是 stale session，被忽略。

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
├── user_id         UUID NULL FK → users.id
├── stream_key      TEXT UNIQUE NOT NULL     ← MediaMTX path (= stream.id 的字串)
├── title           TEXT NULL
├── status          ENUM (Pending, Live, Ended, Error)
├── vod_status      ENUM (None, Processing, Ready, Failed)  DEFAULT None
├── started_at      TIMESTAMPTZ NULL
├── ended_at        TIMESTAMPTZ NULL
├── created_at      TIMESTAMPTZ
├── hls_url         TEXT NULL                ← VOD HLS URL（Ready 後填入）
└── thumbnail_url   TEXT NULL                ← Live / VOD / 自訂縮圖（後進者覆蓋）

recordings
├── id              UUID PK
├── stream_id       UUID FK → streams.id
├── file_path       TEXT NOT NULL            ← 容器內錄影段路徑
├── duration_secs   INTEGER NULL
├── file_size_bytes BIGINT NULL
└── created_at      TIMESTAMPTZ
```

> SPEC-020 已移除 `stream_tokens` table。stream_token 全部改存 Redis（見 §7）。

### Entity 關聯

```
users ──── 1:N ──── streams ──── 1:N ──── recordings
```

Session、token、MTX routing 相關狀態**全部放 Redis**，不進 PostgreSQL。

---

## 7. Redis 使用

Redis 是控制平面的共享狀態 + pub/sub 匯流排。本地開發用單 instance，生產可換成 managed Redis。

### Key schema（所有 key 都必須透過 `crates/mediamtx/src/keys.rs` 產生，不允許 inline format!）

| Key | Value | TTL | 用途 |
|-----|-------|-----|------|
| `stream_token:{sha256(token)}` | `stream_id` (UUID string) | 3600s | Broadcaster WHIP 認證。MediaMTX `/internal/auth` 查這支 key。 |
| `stream:{stream_id}:active_session` | `session_id` (UUID) | 永久（由 end_session 清） | 目前這個 stream 應該在哪個 session 上。publish/unpublish webhook 比對用。 |
| `session:{session_id}:mtx` | `mtx_name` | 永久 | 此 session 住在哪一台 MTX。 |
| `session:{session_id}:stream_id` | `stream_id` | 永久 | 反查。 |
| `session:{session_id}:started_at` | RFC3339 字串 | 永久 | 診斷用。 |
| `mtx:{name}:stream_count` | 整數字串 | 永久 | 該 MTX 目前的 live stream 數，`select_instance` 做 least-streams 決策用。 |
| `mtx:{name}:status` | `healthy` / `unhealthy` / `draining` | healthy 30s / 其餘永久 | Health check + drain 寫入；`select_instance` 只挑 healthy。 |
| `viewer_count_lock` | lock token | 短 TTL | 分散鎖，viewer count 統計時避免多 instance 重複計算。 |
| `health_check_lock` | lock token | 短 TTL | 分散鎖，避免多 API instance 同時打 MTX 的 `/v3/paths/list`。 |
| `chat:{stream_id}:stream` | Redis Stream，每筆欄位 `id`(UUID v7) / `user_id` / `display_name` / `content` | 86400s（每次 XADD 重設） | 直播聊天訊息。`XADD MAXLEN ~1000 *`，`XREVRANGE` 讀最近 50 則做 scrollback。 |
| `chat:ratelimit:{user_id}` | `"1"` | 1s | 聊天發送限流（每 user 每秒 1 則），`SET NX EX 1`。 |
| `chat:{stream_id}:msgindex` | HASH `{uuid_v7 → stream_entry_id}` | 86400s（跟 chat stream 同步） | 訊息刪除用：UUID v7 → Redis Stream entry ID 映射，`HGET` 後 `XDEL`。 |
| `chat:ban:{stream_id}:{user_id}` | `"1"` | EX duration / 永久 | 個別使用者禁言 key。`send_chat` 時 `EXISTS` 快速檢查。 |
| `chat:bans:{stream_id}` | SET of `user_id` | 無 TTL | 禁言索引。`SMEMBERS` 列出該 stream 所有被 ban 的人；過期的 member 在 list 時 lazy `SREM`。 |

### Rate limit key schema

| Key | Value | TTL | 用途 |
|-----|-------|-----|------|
| `ratelimit:{bucket}:{id}` | 整數（counter） | = window 秒數 | Fixed window rate limiting。`bucket` 為 policy 名（`general_authed` / `general_unauthed` / `login` / `register` / `refresh` / `stream_token` / `ws` / `chat` / `bo_general`），`id` 為 `user_id` 或 client IP。Lua script `INCR + EXPIRE`，超過 limit 回 429。 |

### Rate limit middleware pipeline

```
Request
  │
  ├─ /internal/*        → 不限流，直入 handler
  │
  ├─ 有 route-level policy（login / register / refresh / stream_token / ws / chat）
  │   └─ 先跑 route-level check（by IP 或 user_id）→ 通過才進 handler
  │
  └─ 其他路由
      ├─ 已認證（JWT valid）→ general_authed（120/min by user_id）
      └─ 未認證            → general_unauthed（30/min by IP）
```

bo-api 的 rate limit pipeline 較簡單：所有路由統一用 `bo_general`（60/min by user_id）。

### PubSub channel

- `streamhub:events`：`RedisEvent` JSON，由 publish/unpublish handler、drain handler、first live-thumbnail、viewer_count 更新等處 publish；所有 API instance 都訂閱並扇出到各自的 WebSocket 連線。
- `streamhub:chat:{stream_id}`：聊天 JSON（`ServerMessage::ChatMessage` 或 `ChatMessageDeleted`）。`send_chat` 在 XADD 後 PUBLISH；`delete_message` 在 XDEL 後 PUBLISH `chat_message_deleted`。每個 API instance 在第一次本地 `subscribe_chat` 時透過 `ensure_chat_pubsub_task` 懶啟動該房間的 subscriber，將收到的訊息 fan-out 給本地 WS。publisher instance 不做本地 fan-out，避免雙重收到。

### 失效與誤進入 stale 情境

- `create_session` 會**覆寫** `stream:{id}:active_session`，不清舊 session 的 `session:*` keys，交由舊的 webhook 觸發時走 `cleanup_stale_session` 路徑（僅 DECR 原 MTX 計數、刪 session 鍵，不動 DB 與 active_session）。
- `end_session` 清 session keys 時，只有在當下 `active_session` 仍指向自己時才會刪 active_session，避免 race 覆蓋新 session。

---

## 8. 本地開發 Compose 切分

```
deploy/
├── app/             ← Rust API server（public API，port 8080）
│   ├── docker-compose.yml
│   ├── Dockerfile.api
│   └── .env / .env.example
├── bo/              ← Admin backoffice API（port 8800）
│   ├── docker-compose.yml
│   ├── Dockerfile.bo-api
│   └── .env / .env.example
├── infra/           ← 共用基礎設施（nginx + PostgreSQL + Redis + fake-gcs）
│   ├── docker-compose.yml
│   └── nginx.conf
├── media/           ← MediaMTX × 3
│   ├── docker-compose.yml
│   ├── Dockerfile.mediamtx
│   ├── entrypoint.sh    ← 模板替換 + SIGTERM drain
│   └── mediamtx.yml     ← 共用設定模板（__MTX_NAME__ / __WEBRTC_UDP_PORT__）
├── web/             ← 前端靜態 server（broadcaster + viewer + admin，純 HTML/JS）
│   ├── docker-compose.yml
│   ├── Dockerfile.web
│   └── nginx.conf
└── observability/   ← Prometheus / Grafana / Loki / Tempo / Promtail / Alertmanager
    ├── docker-compose.yml
    ├── alertmanager.yml          ← receivers / routes（Telegram，env var 替換）
    └── rules/streamhub.yml       ← alert rules（critical + warning 兩 group）
```

**共用資源：**

- Network：所有 compose 用外部 `streamhub` docker network。
- Volumes（external）：`recordings`（mtx 寫 / api 讀）、`thumbnails`（api 寫）。

**啟動順序建議**：`infra → app → media → web → observability`（infra 提供 network 和 DB；app 先準備好才能接 webhook；media 啟動會去 curl api）。

**Alerting**：observability stack 內含 Alertmanager（port 9093），Prometheus 評估 `rules/*.yml` 觸發 alert 後送往 Alertmanager，receiver 用 Telegram bot（`TELEGRAM_BOT_TOKEN` / `TELEGRAM_CHAT_ID` 從 `deploy/observability/.env` 讀入，entrypoint 用 sed 把 `${VAR}` 替換進 `alertmanager.yml`）。critical（API down / 5xx > 5% / 磁碟 < 10%）立即通知；warning（p95 > 2s / mem > 85% / node-exporter down / Prom 重載失敗）聚合 5 分鐘通知。詳見 SPEC-023。

**nginx 只面對 API 與 web 靜態資源**，MediaMTX 不經過 nginx（broadcaster/viewer 直接連 mtx 的 public port）。

---

## 9. GCP 基礎設施

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

- `Deployment/api`（多副本，共享 Redis PubSub 同步 WS 狀態）
- `Deployment/mediamtx-{1..N}` 或 `StatefulSet/mediamtx`（多 MediaMTX 實例；replicas 依負載調整）
- `Deployment/redis` 或 Memorystore for Redis
- `Service/api`（LoadBalancer，對外）
- `Service/mediamtx-{i}`（LoadBalancer / NodePort，對外；每實例獨立 WebRTC UDP port）
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
| Memorystore for Redis | 生產 Redis，替換本地 `redis:7-alpine` |
| GCS `streamhub-recordings-{env}` | 原始 fMP4 暫存 |
| GCS `streamhub-vod-{env}` | 轉檔後 HLS + spriteSheets 縮圖，CDN origin |
| Cloud CDN | HLS 分發，指向 vod bucket |
| Transcoder API | 非同步多解析度轉檔 |
| Pub/Sub `transcoder-complete` | Transcoder 完成通知 → `/internal/hooks/transcoder-complete` |
| Workload Identity | GKE pod → GCP service 無 key 認證 |
| Cloud Armor | API / MediaMTX LoadBalancer 防護 |

---

## 10. 安全邊界

### 網路邊界

```
Internet
  │
  ├── Cloud Armor (WAF / DDoS)
  │     │
  │     ├── api.streamhub.com              → GKE Service/api (LoadBalancer)
  │     └── mediamtx-{i}.streamhub.com     → GKE Service/mediamtx-{i} (LoadBalancer)
  │                                          （URL 由 API 動態回傳給 client）
  │
  └── cdn.streamhub.com                     → Cloud CDN → GCS vod bucket

GKE internal (ClusterIP):
  mediamtx-{i} → POST http://api-internal:8080/internal/hooks/*
  mediamtx-{i} → POST http://api-internal:8080/internal/auth
  mediamtx-{i} → POST http://api-internal:8080/internal/mtx/drain  (entrypoint SIGTERM trap)
  api          → Redis (cache/pubsub)
  api          → Cloud SQL Auth Proxy → Cloud SQL
  api          → GCS (Workload Identity)
  api          → Transcoder API (Workload Identity)
```

### 認證機制

| 路徑 | 認證方式 |
|------|---------|
| `POST /v1/auth/login` / `/register` / `/refresh` | email + password → JWT |
| 其他 `/v1/*` 需 owner 的路由 | `Authorization: Bearer <JWT>` |
| WHIP 推流 | Token 夾在 WHIP URL 的 `?token=...`，MediaMTX `authMethod: http` 呼叫 `/internal/auth`，API hash 後查 Redis `stream_token:{hash}` |
| `/internal/hooks/*` | 只接受 ClusterIP / docker network，不經過 LoadBalancer |
| `/internal/auth` | 同上，只接受 cluster 內流量 |
| `/internal/mtx/drain` | 同上，由 MediaMTX entrypoint.sh 在 SIGTERM 時呼叫 |
| `/internal/hooks/transcoder-complete` | Pub/Sub push，可選 `pubsub_verify_token`；生產建議改 OIDC |
| GCP services | Workload Identity（無 service account key） |
| Cloud SQL | Cloud SQL Auth Proxy + Workload Identity |

### MediaMTX 認證

```yaml
# mediamtx.yml
authMethod: http
authHTTPAddress: http://api:8080/internal/auth
# MediaMTX 每次 publish / read 會 POST 一個 JSON（含 path, action, query 等），
# API handler：
#   action=publish → 從 query 抽 token → hash → 查 Redis stream_token:{hash}
#                    → 比對查到的 stream_id 是否 == path
#   action=read    → 檢查 stream 存在且 status = Live
#   其他           → 預設放行
```

---

## 11. Crate 依賴關係

### 分層（由下至上）

```
Layer 1 葉子 crate（無 internal 依賴）
├── auth         JWT、stream token hash/generate
├── cache        CacheStore trait、Redis 封裝、InMemoryCache for test
├── entity       SeaORM models
├── rate-limit   Redis Lua script fixed window rate limiting + Axum middleware
├── storage      ObjectStorage trait、GCS / fake-gcs
├── telemetry    init_telemetry（OTel OTLP + Prometheus + JSON log）+ base_http_metrics middleware
└── transcoder   GCP Transcoder API + local ffmpeg + HLS thumbnail

Layer 2 組合 crate
├── repo         → entity                     (UoW + StreamRepo / RecordingRepo / UserRepo)
├── mediamtx     → cache                      (MtxInstance、select_instance、session、HealthChecker)
└── migration    → entity                     (seeds only)

Layer 3 application crate
├── api          → 以上所有                                    (Axum handlers、routes、WS、tasks，port 8080)
└── bo-api       → repo, cache, mediamtx, auth, error, rate-limit, telemetry  (Admin handlers、routes，port 8800)
```

### 依賴圖

```
                    ┌───────┐      ┌────────┐
                    │  api  │      │ bo-api │
                    └───┬───┘      └───┬────┘
   ┌──────┬─────────┬───┤    ┌─────────┤
   ▼      ▼         ▼   ▼    ▼         ▼
┌──────┐┌────────┐┌───────┐┌──────┐ ┌───────┐ ┌─────────┐ ┌────────┐
│ auth ││ error  ││mediamtx││ repo │ │ cache │ │transcoder│ │storage │
└──────┘└────────┘└───┬───┘└──┬───┘ └───────┘ └─────────┘ └────────┘
                      │       │
                      ▼       ▼
                  ┌───────┐┌────────┐
                  │ cache ││ entity │
                  └───────┘└────────┘
```

**原則**：
- `entity`、`auth`、`cache`、`storage`、`transcoder`、`rate-limit`、`telemetry`、`error` 不依賴任何其他 internal crate（可獨立編譯/測試）
- `api` 和 `bo-api` 是 Layer 3 application crate，各自組合底層服務
- 循環依賴一律不允許
