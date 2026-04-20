# SPEC-011a GKE Cluster Bootstrap + App Workloads

狀態：review

## 目標

建立 streamhub 的 GKE regional production 基線，但先只處理：
- cluster / node pool / namespace / IAM / networking 基礎建設
- `api`、`bo-api`、`web` 三個 workload
- `Cloud SQL`、`Memorystore Redis`、`GCS` 接入
- 基本 deploy / rollback pipeline

本 spec 不處理 MediaMTX media plane 細節，也不處理 observability stack 搬遷。

## 定案

### IaC

- `Terraform`：管理 GCP 資源
- `Kustomize`：管理 Kubernetes manifests

不採「腳本或 Terraform 之後再決」。

### CD 基線

MVP 定案：
- CI build image 推到 `Artifact Registry`
- deploy 先用手動觸發的 `kubectl apply -k` / `kustomize build ... | kubectl apply -f -`
- 不在本 spec 引入 ArgoCD

之後若要 GitOps，另開 spec。

### Staging / Prod 拓樸

定案：
- `staging` 與 `prod` 共用同一個 `GKE Standard regional cluster`
- 以 namespace 隔離：`streamhub-staging`、`streamhub-prod`
- staging workload 使用獨立 overlay，但不建第二個 cluster

原因：
- 避免多一份 regional cluster management fee
- 初版先把平台複雜度壓低
- control plane blast radius 可接受，之後若 staging 使用量或風險升高，再拆獨立 cluster

### Cloud SQL

定案：
- `Cloud SQL private IP`
- `Private Service Access`（allocated IP range + VPC peering）納入 Terraform
- GKE workload 直接經 VPC 連線
- 不用 Cloud SQL Auth Proxy sidecar / DaemonSet

原因：
- 拓樸更簡單
- 少一層 sidecar 維運與資源成本
- 與 production 長期方向一致

### 證書管理

定案：
- `Google-managed certificate`
- 搭配 GKE Gateway / Ingress 使用

不在本 spec 導入 `cert-manager`。

### Secret 注入

定案：
- `External Secrets Operator`
- source of truth：`Secret Manager`
- workload 透過 `Workload Identity` 讀取 Secret Manager

不採：
- Secret Manager CSI driver
- init container 自行抓 secret

原因：
- 與 Kubernetes Secret 使用習慣相容
- manifest 介面清楚
- 比 init container 自行取值更容易維護與審計

### Admin domain 安全

- `admin.streamhub.com` 獨立 host
- 與 public API 分開的 `Cloud Armor` policy
- 本 spec 先不導入 IAP
- 若後續需要企業內控，再另開 spec 評估 IAP

### HPA 指標

定案基線：
- `api`：CPU utilization target `70%`
- `bo-api`：CPU utilization target `70%`
- `web`：CPU utilization target `70%`

後續若要加 memory / custom metrics，另開調校任務。

### Deploy 入口

定案：
- 提供 `deploy/gke/scripts/deploy.sh`
- 內部實作可呼叫 `kubectl apply -k`
- human entrypoint 統一是這支 script，不要求操作人員直接手打多條 kubectl 指令

## 非目標

- 不處理 MediaMTX StatefulSet / UDP / per-pod host
- 不處理 Grafana / Loki / Tempo on GKE
- 不處理 staging smoke 與完整 prod runbook

## 影響範圍

新增：
- `deploy/gke/base/namespace-*.yaml`
- `deploy/gke/base/api-*.yaml`
- `deploy/gke/base/bo-api-*.yaml`
- `deploy/gke/base/web-*.yaml`
- `deploy/gke/base/gateway-*.yaml`
- `deploy/gke/base/networkpolicy-*.yaml`
- `deploy/gke/base/serviceaccount-*.yaml`
- `deploy/gke/prod/`
- `deploy/gke/staging/`
- `deploy/gke/scripts/`
- `infra/terraform/` 或等價目錄

修改：
- `deploy/app/.env.example`
- `deploy/bo/.env.example`
- `docs/deploy.md`
- `docs/architecture.md`

## Kubernetes 基線

### Namespaces

- `streamhub-prod`
- `streamhub-staging`
- `observability`

### Node pools

至少三組：
- `app` node pool：`api`、`bo-api`、`web`
- `media` node pool：留給 `011b`
- `ops` node pool：留給 `011c`

### App replicas / HPA

- `api`: min `3`, max `10`
- `bo-api`: min `2`, max `5`
- `web`: min `2`, max `4`

### Zone spread

`api`、`bo-api`、`web` 都要有 `topologySpreadConstraints`，避免 regional cluster 退化成單 zone 排程。

## Todo list

- [x] SPEC-011a-01 Terraform 建 GKE regional cluster + node pools + VPC 相關資源
- [x] SPEC-011a-02 Terraform 建 `Private Service Access`、Cloud SQL private IP、Memorystore、GCS、Artifact Registry
- [x] SPEC-011a-03 建 `streamhub-prod` / `streamhub-staging` namespaces
- [x] SPEC-011a-04 建 `api` Deployment / Service / HPA / PDB / topology spread manifests
- [x] SPEC-011a-05 建 `bo-api` Deployment / Service / HPA / PDB / topology spread manifests
- [x] SPEC-011a-06 建 `web` Deployment / Service / HPA / PDB / topology spread manifests
- [x] SPEC-011a-07 建 Gateway / Ingress / Google-managed certificate / domain routing manifests
- [x] SPEC-011a-08 建 Workload Identity + External Secrets Operator + Secret Manager 注入方案
- [x] SPEC-011a-09 建 NetworkPolicy，限制 DB / Redis / internal traffic
- [x] SPEC-011a-10 建 `deploy/gke/scripts/deploy.sh` 與手動 deploy / rollback pipeline
- [x] SPEC-011a-11 docs 更新：deploy + architecture

## 驗收標準

- regional cluster 可建立並可部署 `api` / `bo-api` / `web`
- `api.streamhub.com`、`admin.streamhub.com`、`web.streamhub.com` 可正常提供服務
- `Cloud SQL`、`Memorystore`、`GCS` 連線正常
- 不依賴 service account JSON key
- app workloads 在單一 zone 故障時仍可維持服務
