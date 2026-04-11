# SPEC-011 GCE 部署

狀態：draft

## 目標

用 Cloud Load Balancer + GCE Managed Instance Group 部署 streamhub。
每台 VM 跑 Docker Compose（nginx + API + MediaMTX），DB 用 Cloud SQL。
支援 rolling update 不中斷直播。

## 架構

```
Internet
    │
Cloud Load Balancer
    ├── HTTPS/443 → GCE MIG (nginx:3000)
    ├── UDP/8189  → GCE MIG (WebRTC ICE)
    └── Health check → /api/v1/health
    │
GCE Managed Instance Group (MIG)
    ├── Instance Template
    │   ├── Container-Optimized OS (COS)
    │   ├── docker-compose.yml
    │   │   ├── nginx     (前端 + 反向代理)
    │   │   ├── api       (Rust API server)
    │   │   ├── mediamtx  (媒體路由)
    │   │   └── fake-gcs  (僅 dev，prod 移除)
    │   └── startup-script.sh (pull image + docker compose up)
    │
Cloud SQL (PostgreSQL 17)
    │
GCS Bucket (streamhub-recordings-{env})
```

## 影響範圍

新增：
- `deploy/gce/` — GCE 部署相關檔案
  - `instance-template.sh` — 建立 instance template 的 gcloud 指令
  - `startup-script.sh` — VM 啟動時執行（pull images + docker compose up）
  - `setup-lb.sh` — 建立 Load Balancer + health check
  - `setup-cloudsql.sh` — 建立 Cloud SQL instance
  - `setup-gcs.sh` — 建立 GCS bucket + IAM
  - `deploy.sh` — 一鍵部署腳本
- `deploy/docker-compose.prod.yml` — 正式環境用（移除 fake-gcs、postgres，改用 Cloud SQL）
- `deploy/.env.prod.example` — 正式環境 env 範本

修改：
- `deploy/docker-compose.yml` — 加 health check endpoint
- `crates/api/src/routes.rs` — 加 GET /api/v1/health endpoint
- `deploy/nginx.conf` — SSL 由 LB 處理，nginx 只 listen 80
- `deploy/mediamtx.yml` — webrtcAdditionalHosts 設為 VM 外部 IP

## Todo list

- [ ] SPEC-011-01 API health endpoint — GET /api/v1/health（回 200，給 LB health check 用）
- [ ] SPEC-011-02 docker-compose.prod.yml — 正式環境版本（無 postgres、無 fake-gcs、Cloud SQL 連線）
- [ ] SPEC-011-03 .env.prod.example — 正式環境 env 範本（Cloud SQL URL、real GCS bucket、Transcoder 設定）
- [ ] SPEC-011-04 deploy/gce/setup-cloudsql.sh — 建立 Cloud SQL PostgreSQL 17 instance
- [ ] SPEC-011-05 deploy/gce/setup-gcs.sh — 建立 GCS bucket + Service Account + IAM 權限
- [ ] SPEC-011-06 deploy/gce/startup-script.sh — VM 開機腳本（安裝 docker compose、pull images、啟動服務）
- [ ] SPEC-011-07 deploy/gce/instance-template.sh — 建立 GCE instance template（machine type、disk、network tag、metadata）
- [ ] SPEC-011-08 deploy/gce/setup-lb.sh — 建立 TCP Load Balancer + UDP Load Balancer + health check + firewall rules
- [ ] SPEC-011-09 deploy/gce/setup-mig.sh — 建立 Managed Instance Group + autoscaling policy
- [ ] SPEC-011-10 deploy/gce/deploy.sh — 一鍵部署（build + push image + rolling update MIG）
- [ ] SPEC-011-11 mediamtx.yml — webrtcAdditionalHosts 從環境變數讀取（VM 外部 IP）
- [ ] SPEC-011-12 nginx.conf prod 版 — SSL 由 LB termination，nginx listen 80，移除 ssl 設定
- [ ] SPEC-011-13 驗證 — 本地 docker-compose.prod.yml 可啟動（接 Cloud SQL）

## 關鍵設計

### Docker Image Registry

用 Artifact Registry（GCR 的替代）：
```
asia-east1-docker.pkg.dev/{project}/streamhub/api:latest
asia-east1-docker.pkg.dev/{project}/streamhub/web:latest
asia-east1-docker.pkg.dev/{project}/streamhub/mediamtx:latest
```

### Instance Template

```bash
gcloud compute instance-templates create-with-container streamhub-template \
  --machine-type=e2-medium \
  --boot-disk-size=20GB \
  --tags=streamhub,http-server,https-server \
  --metadata-from-file=startup-script=startup-script.sh \
  --scopes=cloud-platform \
  --service-account=streamhub-vm@{project}.iam.gserviceaccount.com
```

### Load Balancer

```
TCP Proxy LB (HTTPS)
  ├── Frontend: 0.0.0.0:443 (SSL cert)
  ├── Backend: GCE MIG, port 3000
  └── Health check: TCP 3000, path /api/v1/health

Network LB (UDP)
  ├── Frontend: 0.0.0.0:8189 (UDP)
  └── Backend: GCE MIG, port 8189
```

### Rolling Update 不中斷直播

```bash
# MIG 設定
gcloud compute instance-groups managed set-update-policy streamhub-mig \
  --type=proactive \
  --max-surge=1 \
  --max-unavailable=0 \
  --replacement-method=substitute \
  --minimal-action=replace
```

- `max-unavailable=0`：永遠不會同時關掉所有 instance
- `max-surge=1`：先開一台新的，ready 後才關舊的
- 搭配 connection draining：舊 instance 等現有連線結束才 shutdown

### MediaMTX WebRTC ICE

VM 啟動時要知道自己的外部 IP：
```bash
# startup-script.sh 裡
EXTERNAL_IP=$(curl -s -H "Metadata-Flavor: Google" http://metadata.google.internal/computeMetadata/v1/instance/network-interfaces/0/access-configs/0/external-ip)
export MEDIAMTX_ADDITIONAL_HOSTS=$EXTERNAL_IP
```

mediamtx.yml 加：
```yaml
webrtcAdditionalHosts:
  - $MEDIAMTX_ADDITIONAL_HOSTS
```

### Cloud SQL 連線

VM 透過 private IP 連 Cloud SQL（同 VPC）：
```
DATABASE_URL=postgres://streamhub:xxx@10.x.x.x:5432/streamhub
```

不用 Cloud SQL Auth Proxy，直接 private IP。

## 驗收標準

- `deploy.sh` 一鍵建立完整環境（Cloud SQL + GCS + MIG + LB）
- 從外部 IP 可以存取 https://{LB_IP}/
- 推流 + 觀看 + VOD 正常
- Rolling update 不中斷正在進行的直播
- Autoscaling 在 CPU > 70% 時自動加機器

## 備註

- 先用 HTTP LB（不上 SSL），SSL 後續加 managed cert
- Autoscaling 初始 min=1, max=3
- Machine type 初始 e2-medium（2 vCPU, 4GB RAM），可依負載調整
- 預估每月費用：~$30（1 台 e2-medium）+ ~$15（Cloud SQL db-f1-micro）+ GCS 用量
