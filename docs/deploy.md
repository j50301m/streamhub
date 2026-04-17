# Deploy Runbook

本文件說明如何在本地起一套完整的 streamhub 環境，以及常見故障排除。
目標：**照著複製貼上指令，就能跑起可用環境**。

GCP 部署仍在規劃中（SPEC-011），本文件只提供方向性 skeleton，實際流程待 SPEC-011 完成後補上。

---

## 1. 前置需求

| 工具 | 版本 | 用途 |
|---|---|---|
| Docker Desktop（macOS）或 Docker Engine + compose plugin | 最新穩定 | 跑所有服務 |
| Rust | 1.85+（MSRV，edition 2024） | `cargo build` / `cargo run -p api` |
| `just` | 1.x | 跑 `just check` / `just up` 等快捷指令 |
| `gcloud` CLI | 最新 | **可選**，只有部署 GCP 或使用真實 GCS bucket 才需要 |
| `redis-cli` | 7.x | **可選**，除錯用 |
| `psql` | 17.x | **可選**，除錯用 |

## 2. 首次啟動（Bootstrap）

### 2.1 複製設定檔

```bash
cd /path/to/streamhub
cp deploy/app/.env.example deploy/app/.env
cp deploy/bo/.env.example deploy/bo/.env
```

預設值可直接跑本地開發。正式環境務必改：

- `JWT_SECRET`（隨機 32+ 字元，**api 和 bo-api 的 `.env` 必須一致**）
- DB 帳密（`DATABASE_URL` 內）
- `GCS_BUCKET` / `GCS_ENDPOINT`（正式環境刪掉 `GCS_ENDPOINT`，改用真實 GCS）

### 2.2 一鍵啟動（推薦）

`justfile` 已封裝 network / volume 建立與 compose 啟動順序：

```bash
just up-all      # = just up + just up-obs
```

這會：

1. 建 external network `streamhub`（所有 compose 共用）
2. 建 external volumes `recordings`、`thumbnails`
3. 依序 up：`infra` → `app` → `bo` → `media` → `web` → `observability`

### 2.3 手動啟動（了解順序）

若不用 just，等效指令：

```bash
# 共享資源（external network / volumes）
docker network create streamhub
docker volume create recordings
docker volume create thumbnails

# 啟動順序：infra（DB / Redis / fake-gcs / nginx）先起
docker compose -f deploy/infra/docker-compose.yml up -d

# API（依賴 postgres / redis / fake-gcs，會透過 docker DNS 連）
docker compose -f deploy/app/docker-compose.yml up --build -d

# bo-api（admin backoffice API，依賴 postgres / redis）
docker compose -f deploy/bo/docker-compose.yml up --build -d

# MediaMTX 3 實例（webhook 打 api:8080）
docker compose -f deploy/media/docker-compose.yml up --build -d

# 靜態網頁（broadcaster + viewer，nginx 代理）
docker compose -f deploy/web/docker-compose.yml up --build -d

# 觀測性（獨立 stack，可選）
docker compose -f deploy/observability/docker-compose.yml up -d
```

### 2.4 驗證

```bash
# 檢查 container
docker ps --filter network=streamhub

# API healthcheck
curl http://localhost:8080/health

# 前端入口（nginx 在 infra 的 3000 port）
open http://localhost:3000/

# Grafana（觀測性的 3001 port）
open http://localhost:3001/
```

### 2.5 停止 / 清除

```bash
just down-all                          # 停全部
just down                              # 只停 app/media/web/infra（保留觀測）
docker volume rm recordings thumbnails # 清除錄檔（小心！）
```

---

## 3. 本地開發循環

### 方案 A：全 compose（貼近 prod）

適合 review 完整系統行為：

```bash
# 改完 code 後 rebuild API container
docker compose -f deploy/app/docker-compose.yml up --build -d
docker compose -f deploy/app/docker-compose.yml logs -f api
```

### 方案 B：infra/media/observability 用 compose，API 跑 cargo（開發推薦）

快速迭代 API 修改：

```bash
# 1. 起 infra + media + observability（不起 app）
docker compose -f deploy/infra/docker-compose.yml up -d
docker compose -f deploy/media/docker-compose.yml up --build -d
docker compose -f deploy/observability/docker-compose.yml up -d

# 2. 匯出本地開發用 env（host port，不是 container DNS）
export DATABASE_URL=postgres://streamhub:streamhub@localhost:5433/streamhub
export REDIS_URL=redis://localhost:6381
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4319
export GCS_ENDPOINT=http://localhost:4443
export RECORDINGS_PATH=./recordings
export MEDIAMTX_INSTANCES='[{"name":"mtx-1","internal_api":"http://localhost:9997","public_whip":"http://localhost:8889","public_whep":"http://localhost:8889","public_hls":"http://localhost:8888"}]'

# 3. 跑 API
cargo run -p api
```

注意：方案 B 下 MediaMTX webhook 打的是 `http://api:8080`（docker network 內），
若 API 跑在 host，MediaMTX 連不到。要驗證 webhook 請用方案 A。

### 方案 C：前端直接開

`web/broadcaster/` 和 `web/viewer/` 是純 HTML/JS，不用 build：

```bash
open web/broadcaster/index.html
open web/viewer/index.html
```

但推薦走 nginx 代理（http://localhost:3000/broadcaster/ 等）避免 CORS 問題。

---

## 4. 端口對照表

### API / 前端

| 服務 | host port | 內部 port | 用途 |
|---|---|---|---|
| api | 8080 | 8080 | HTTP + WebSocket（public API） |
| bo-api | 8800 | 8800 | HTTP（admin backoffice API） |
| nginx（infra） | 3000 | 3000 | 前端總入口（代理 /api/、/v1/、/ws、/broadcaster/、/viewer/、/admin/、/vod/） |
| web | —（僅 expose） | 8080 | 靜態檔案，由 nginx 代理 |

### 基礎設施

| 服務 | host port | 內部 port | 備註 |
|---|---|---|---|
| postgres | 5433 | 5432 | 本地 host 是 5433 避免跟系統 Postgres 衝突 |
| redis | 6381 | 6379 | 本地 host 是 6381 避免衝突 |
| fake-gcs | 4443 | 4443 | 本地取代 GCS，`GCS_ENDPOINT` 指到這 |

### MediaMTX（3 實例）

| 服務 | RTSP | HLS | WebRTC HTTP | WebRTC UDP（內外一致） |
|---|---|---|---|---|
| mtx-1 | 8554 | 8888 | 8889 | 8189 |
| mtx-2 | 8564 | 8898 | 8899 | 8199 |
| mtx-3 | 8574 | 8908 | 8909 | 8209 |

> WebRTC UDP 內外 port **必須一致**（8189 / 8199 / 8209），否則 ICE candidate
> 指向的 port 跟 host 映射不同，WebRTC 連線會失敗。

MediaMTX API port（`9997`）不對外開放，只在 docker network 內部供 API 呼叫。

### 觀測性

| 服務 | host port | 內部 port | 用途 |
|---|---|---|---|
| grafana | 3001 | 3000 | UI（匿名 Admin） |
| prometheus | 9091 | 9090 | metrics |
| tempo | 3200 | 3200 | tracing HTTP |
| tempo OTLP | 4319 | 4317 | OTLP gRPC（API 送 trace 進來） |
| loki | 3100 | 3100 | logs |
| node-exporter | 9100 | 9100 | 主機 metrics |
| promtail | — | — | 只讀 docker.sock，沒對外 port |
| alertmanager | 9093 | 9093 | alert routing → Telegram |

---

## 5. 環境變數對照

以下對應 `crates/api/src/config.rs`（API）和 `crates/bo-api/src/config.rs`（bo-api）實際讀取的 key。所有變數都有 default，
未標「必填」的可留空。API 讀取來源：`deploy/app/.env`（被 `deploy/app/docker-compose.yml`
的 `env_file: .env` 載入）；bo-api 讀取來源：`deploy/bo/.env`。

### Database / Redis

| 變數 | 預設 | 必填 | 說明 |
|---|---|---|---|
| `DATABASE_URL` | `postgres://streamhub:streamhub@localhost:5433/streamhub` | ✗ | container 內用 `postgres:5432`（docker DNS） |
| `REDIS_URL` | `redis://localhost:6379` | ✗ | container 內用 `redis:6379` |

### HTTP Server / JWT

| 變數 | 預設 | 必填 | 說明 |
|---|---|---|---|
| `HOST` | `0.0.0.0` | ✗ | API bind 位址 |
| `PORT` | `8080` | ✗ | API port |
| `JWT_SECRET` | `dev-secret-change-in-production` | ⚠ prod 必填 | JWT 簽章 secret，正式環境務必替換 |

### 檔案路徑

| 變數 | 預設 | 說明 |
|---|---|---|
| `RECORDINGS_PATH` | `./recordings` | fMP4 錄檔路徑（容器內通常是 `/recordings`） |
| `THUMBNAILS_PATH` | `/thumbnails` | 直播預覽圖路徑 |

### GCS

| 變數 | 預設 | 必填 | 說明 |
|---|---|---|---|
| `GCS_BUCKET` | `streamhub-recordings-dev` | ✗ | 錄檔 bucket 名 |
| `GCS_ENDPOINT` | `""` | ✗ | 本地用 `http://fake-gcs:4443`；prod 留空走真 GCS |
| `GCS_CREDENTIALS_PATH` | `""` | ⚠ 非 WIF 時必填 | Service account JSON path；GKE 上用 Workload Identity 可留空 |

### Transcoder / Pub/Sub

| 變數 | 預設 | 說明 |
|---|---|---|
| `TRANSCODER_ENABLED` | `false` | `true` / `1` 啟用（本地通常關） |
| `TRANSCODER_PROJECT_ID` | `""` | GCP project id（啟用時必填） |
| `TRANSCODER_LOCATION` | `asia-east1` | Transcoder API region |
| `PUBSUB_VERIFY_TOKEN` | `""` | Pub/Sub push endpoint 驗證 token |

### MediaMTX

| 變數 | 預設 | 必填 | 說明 |
|---|---|---|---|
| `MEDIAMTX_INSTANCES` | `""` | ✓ | JSON array，每個 instance 含 `name` / `internal_api` / `public_whip` / `public_whep` / `public_hls` |

範例（本地 3 實例）見 `deploy/app/.env.example`。

### Rate Limiting（API）

所有 limit / window 值皆可透過環境變數覆蓋。讀取自 `deploy/app/.env`。

| 變數 | 預設 | 說明 |
|---|---|---|
| `RATE_LIMIT_GENERAL_AUTHED_LIMIT` | `120` | 已認證一般請求上限 |
| `RATE_LIMIT_GENERAL_AUTHED_WINDOW` | `60` | 已認證一般請求窗口（秒） |
| `RATE_LIMIT_GENERAL_UNAUTHED_LIMIT` | `30` | 未認證一般請求上限 |
| `RATE_LIMIT_GENERAL_UNAUTHED_WINDOW` | `60` | 未認證一般請求窗口（秒） |
| `RATE_LIMIT_LOGIN_LIMIT` | `5` | 登入上限 |
| `RATE_LIMIT_LOGIN_WINDOW` | `900` | 登入窗口（秒，15min） |
| `RATE_LIMIT_REGISTER_LIMIT` | `5` | 註冊上限 |
| `RATE_LIMIT_REGISTER_WINDOW` | `900` | 註冊窗口（秒，15min） |
| `RATE_LIMIT_REFRESH_LIMIT` | `10` | Refresh token 上限 |
| `RATE_LIMIT_REFRESH_WINDOW` | `60` | Refresh token 窗口（秒） |
| `RATE_LIMIT_STREAM_TOKEN_LIMIT` | `5` | 串流 token 上限 |
| `RATE_LIMIT_STREAM_TOKEN_WINDOW` | `60` | 串流 token 窗口（秒） |
| `RATE_LIMIT_WS_LIMIT` | `10` | WebSocket 連線上限 |
| `RATE_LIMIT_WS_WINDOW` | `60` | WebSocket 連線窗口（秒） |
| `RATE_LIMIT_CHAT_LIMIT` | `1` | 聊天發送上限 |
| `RATE_LIMIT_CHAT_WINDOW` | `1` | 聊天發送窗口（秒） |

### Bo-API 環境變數

讀取自 `deploy/bo/.env`。

| 變數 | 預設 | 必填 | 說明 |
|---|---|---|---|
| `DATABASE_URL` | `postgres://streamhub:streamhub@postgres:5432/streamhub` | ✗ | 同 api |
| `REDIS_URL` | `redis://redis:6379` | ✗ | 同 api |
| `JWT_SECRET` | — | ⚠ | **必須與 api 的 JWT_SECRET 一致** |
| `BO_API_HOST` | `0.0.0.0` | ✗ | bo-api bind 位址 |
| `BO_API_PORT` | `8800` | ✗ | bo-api port |
| `BO_API_CORS_ORIGINS` | `http://localhost:3000` | ✗ | 允許的 CORS origins |
| `RATE_LIMIT_BO_GENERAL_LIMIT` | `60` | ✗ | bo-api 一般請求上限 |
| `RATE_LIMIT_BO_GENERAL_WINDOW` | `60` | ✗ | bo-api 一般請求窗口（秒） |

### 觀測性 / 輪詢

| 變數 | 預設 | 說明 |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | container 內用 `http://tempo:4317` |
| `THUMBNAIL_CAPTURE_INTERVAL_SECS` | `60` | 預覽圖擷取間隔 |
| `VIEWER_COUNT_INTERVAL_SECS` | `10` | WebSocket 推送觀眾數間隔 |

### Alerting（Telegram）

讀取自 `deploy/observability/.env`（被 alertmanager container 的 `env_file` 載入），entrypoint
用 sed 把 `alertmanager.yml` 裡的 `${VAR}` 替換成實值。範例值在 `deploy/observability/.env.example`。

| 變數 | 預設 | 說明 |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | `dummy:dummy` | BotFather 發的 token，格式 `<id>:<secret>` |
| `TELEGRAM_CHAT_ID` | `1` | 收通知的 chat / group id（int64，非 0） |

> Alertmanager v0.27+ 拒絕 `chat_id: 0`，dummy 也要給非零值才能啟動。
> 真實值從 `@BotFather` 取得 token，從 `https://api.telegram.org/bot<TOKEN>/getUpdates` 取 chat_id。

#### Alerting 手動測試

```bash
# 1. 起 observability stack（含 alertmanager）
cd deploy/observability && cp .env.example .env
just up-obs

# 2. 確認 alertmanager 活著
curl -s http://localhost:9093/api/v2/status | jq '.uptime'

# 3. 確認 prometheus 載入 7 條 alert rule
curl -s http://localhost:9091/api/v1/rules \
  | jq '[.data.groups[].rules[]] | length'   # 應為 7

# 4. 模擬 ApiDown：停掉 API 等 1 分鐘
docker stop streamhub-api
sleep 80
curl -s http://localhost:9093/api/v2/alerts | jq '.[].labels.alertname'
# 應看到 "ApiDown"

# 5. 還原
docker start streamhub-api
```

> 以上用 dummy token 也能跑，Alertmanager 會 log Telegram 推送失敗但不會崩。
> 真實推送驗證請填入真 token 後重做步驟 4，手機應收到通知。

---

## 6. MediaMTX 整合細節

### 6.1 Webhook 走 docker 內部 DNS

`deploy/media/mediamtx.yml` 的 webhook URL 寫死 `http://api:8080/...`：

- `runOnReady` / `runOnNotReady` → `POST /internal/hooks/publish`
- `runOnRecordSegmentComplete` → `POST /internal/hooks/recording`
- `authHTTPAddress` → `http://api:8080/internal/auth`

這些全部走 docker network `streamhub` 的內部 DNS，`api` 解析到 API container。
方案 B（host 跑 cargo）時 MediaMTX 連不到 API，請用方案 A 驗證 webhook。

### 6.2 SIGTERM Drain

`deploy/media/entrypoint.sh` trap SIGTERM：

1. `docker stop streamhub-mtx-N` 觸發 SIGTERM
2. entrypoint curl `POST http://api:8080/internal/mtx/drain?mtx=mtx-N`（標記此 instance 停止接新流）
3. sleep 5s 等 API 把流遷走
4. 才真的 `kill -TERM` MediaMTX

同時 `just drain <mtx>` 可手動觸發 drain（不殺 container）。

### 6.3 WebRTC UDP 內外 Port 必須一致

MediaMTX 的 `webrtcLocalUDPAddress` 會被寫進 ICE candidate，客戶端會直接去連這個 port。
Docker port mapping 若是 `9999:8189`，客戶端連 8189 但 host 沒開，ICE 直接失敗。

因此 compose 裡三個 instance 都是 `8189:8189/udp`、`8199:8199/udp`、`8209:8209/udp`
（內外一致）。entrypoint.sh 用 `WEBRTC_UDP_PORT` 環境變數在啟動時把 `mediamtx.yml`
的 `__WEBRTC_UDP_PORT__` placeholder 替換掉。

### 6.4 `webrtcAdditionalHosts`

本地開發不設（用 `localhost` / `127.0.0.1` 即可）。公網部署時必須在 `mediamtx.yml`
加上 `webrtcAdditionalHosts: [<公網 IP 或 domain>]`，否則 ICE candidate 會回 docker
內部 IP，瀏覽器連不上。

---

## 7. 端到端測試

### 7.1 手動推流 + 觀看

```bash
# 1. 確認環境跑起來
just up-all
curl http://localhost:8080/health

# 2. 建 broadcaster 帳號 + 登入（透過 API，詳 docs/api.md）
#    拿到 JWT，存 localStorage

# 3. 開 broadcaster 頁面
open http://localhost:3000/broadcaster/

# 4. UI 上建 stream → 取 publish token → 開始推流（WHIP 打 mtx-X）

# 5. 另開 viewer 頁面
open http://localhost:3000/viewer/
#    WHEP / HLS 任一模式觀看

# 6. Admin dashboard（需 admin 角色帳號）
open http://localhost:3000/admin/

# 6. 驗證
docker exec streamhub-redis redis-cli keys 'session:*'        # Redis 有 session
docker exec streamhub-postgres psql -U streamhub -d streamhub \
  -c "SELECT id, status FROM stream WHERE status='live';"     # DB status = live
open http://localhost:3001/                                   # Grafana 看 metrics/traces/logs
```

### 7.2 Drain 演練（Multi-MTX 切換）

驗證 `SPEC-017` 的 session-based stream lifecycle：

```bash
# 推流到 mtx-1 後，另開 terminal
docker stop streamhub-mtx-1

# 預期行為：
# - entrypoint.sh SIGTERM trap 觸發 drain webhook
# - API 把此 instance 標記為 draining
# - broadcaster 前端偵測斷線 → 重新 POST /token → 拿到 mtx-2 或 mtx-3
# - viewer 也自動 reconnect 到新的 MTX

# 恢復
docker compose -f deploy/media/docker-compose.yml up -d mtx-1
```

---

## 8. 常見問題 / 故障排除

### Port 已被占用

**不要直接 kill**，先看是誰：

```bash
lsof -i :8080        # macOS / Linux
```

可能是其他專案、其他 Postgres、或上次沒乾淨停掉的 container。由使用者決定處理方式
（停競爭服務，或改本專案的 host port）。

### WebRTC ICE timeout

1. 確認 `mtx-N` compose 的 UDP port 內外一致（`8189:8189/udp` 不是 `9999:8189/udp`）
2. 公網部署：`mediamtx.yml` 加 `webrtcAdditionalHosts`
3. NAT 環境：STUN 不夠，需要 TURN（自架 coturn，SPEC-011 會處理）

### `host.docker.internal` 在 Linux 不存在

macOS Docker Desktop 自動提供，Linux 需要：

```yaml
# compose.yml
services:
  xxx:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

或用 compose `host` 網路模式（僅 Linux）。

### API 重啟後找不到 mtx

API 用 `hickory-dns` resolver 處理 docker DNS TTL，若還出現解析失敗：

1. 確認 `reqwest` 有啟用 `hickory-dns` feature
2. `docker inspect streamhub | grep IPAM` 看 network 是否健康
3. 最後手段：`just down-all && just up-all`

### 被 `docker network prune` 殺掉 streamhub network

```bash
just down-all                                        # 停全部
docker network create streamhub                      # 重建
docker volume create recordings thumbnails 2>/dev/null || true
just up-all                                          # 重起
```

所有 compose 都必須重 `up`，因為容器還掛在舊 network id 上。

### `recordings` volume 爆滿

```bash
docker volume ls | grep recordings
docker run --rm -v recordings:/data alpine du -sh /data
# 清舊錄檔（小心！）
docker run --rm -v recordings:/data alpine sh -c 'find /data -mtime +7 -delete'
```

### Grafana 看不到 trace

1. 確認 API `OTEL_EXPORTER_OTLP_ENDPOINT=http://tempo:4317`（container 內）
   或 `http://localhost:4319`（host 跑時）
2. OTel provider drop 問題（已修）— 若復發，確認 API 是正常 `SIGTERM` 關閉，
   不是被 `kill -9`，否則 span 來不及 flush
3. Tempo 版本鎖 `2.7.1`（不是 latest，避免 Kafka 依賴，見 memory `project_tempo_kafka`）

### Postgres 連線失敗 (`localhost:5432`)

本地 host port 是 **5433**（避免跟系統 Postgres 衝突）。`DATABASE_URL` 在 host 跑
`cargo run` 時要用 `localhost:5433`；在 container 內用 `postgres:5432`。

---

## 9. GCP 部署（Skeleton，待 SPEC-011 完成）

目前 SPEC-011 仍 draft，以下只是規劃方向：

- **Compute**：GKE Autopilot（`asia-east1`）
- **DB**：Cloud SQL PostgreSQL 17
- **Cache**：Memorystore Redis
- **Storage**：GCS bucket `streamhub-recordings-prod`（錄檔）+ Cloud CDN 前置
- **Transcoder**：GCP Transcoder API
- **Auth**：Workload Identity（Pod 身份 → GSA → GCS / Transcoder）
- **Ingress**：Google Cloud Load Balancer + managed cert
- **MediaMTX**：DaemonSet on GKE node（需要穩定 UDP），`webrtcAdditionalHosts` 設 node 外部 IP 或 LB IP
- **TURN**：coturn on GCE
- **Observability**：Cloud Logging / Cloud Monitoring，或維持自管 Grafana stack

詳細步驟（Terraform / manifests / secret 管理）追蹤於 **SPEC-011**。本文件在 SPEC-011
完成後會補齊第 9 章。
