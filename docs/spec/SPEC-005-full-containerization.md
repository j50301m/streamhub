# SPEC-005 完整容器化部署

狀態：review

## 目標

一個 `docker compose up` 啟動整個平台（PostgreSQL + MediaMTX + API + 靜態前端），不需要在 host 上跑 cargo run。

## 影響範圍

新增 / 修改：
- `deploy/Dockerfile.api` — Rust API multi-stage build
- `deploy/docker-compose.yml` — 加 api service + web service
- `deploy/Dockerfile.web` — nginx 靜態前端
- `deploy/nginx.conf` — 前端 nginx 設定
- `web/broadcaster/index.html` — API URL 改為相對路徑或可設定
- `web/viewer/index.html` — 同上
- `deploy/mediamtx.yml` — webhook URL 改為 docker network 內部地址

## Todo list

- [x] SPEC-005-01 deploy/Dockerfile.api — multi-stage build（builder + runtime），輸出精簡 image
- [x] SPEC-005-02 docker-compose.yml — 加 api service（連 postgres、共用 recordings volume、環境變數設定）
- [x] SPEC-005-03 mediamtx.yml — webhook URL 從 host.docker.internal 改為 docker network 的 api:8080
- [x] SPEC-005-04 deploy/nginx.conf + Dockerfile.web — nginx serve 靜態前端 + 反向代理 API
- [x] SPEC-005-05 docker-compose.yml — 加 web service（nginx，port 3000）
- [x] SPEC-005-06 web 前端 — API URL 和 MediaMTX URL 改為透過 nginx 反向代理的相對路徑或可設定
- [x] SPEC-005-07 驗證 — docker compose up --build 全部啟動，端到端測試

## 驗收流程

```bash
cd deploy
docker compose up --build -d

# 全部容器 healthy 後
# 1. 開 http://localhost:3000/broadcaster/ → 註冊 → 建流 → 推流
# 2. 開 http://localhost:3000/viewer/ → 看到 live list → 點擊觀看
# 3. 停止推流 → 確認錄影記錄
```

## 備註

- API Dockerfile 用 multi-stage：stage 1 cargo build --release，stage 2 用 debian-slim 或 alpine
- DATABASE_URL 用 docker network 內部地址：postgres://streamhub:streamhub@postgres:5432/streamhub
- RECORDINGS_PATH 設為 /recordings（容器內路徑，API 和 MediaMTX 共用 volume）
- nginx 反向代理：/api/* → api:8080，/mediamtx/* → mediamtx:8889，靜態檔直接 serve
- 前端的 API_URL 和 MEDIAMTX_URL 需要能從瀏覽器存取，所以走 nginx 反向代理
