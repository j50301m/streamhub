# SPEC-035 bo-api Observability（OpenTelemetry parity with api）

狀態：in-progress

## 目標

1. bo-api 補齊與 api 同等的觀測性：OpenTelemetry traces（OTLP gRPC → Tempo）、Prometheus metrics、JSON logs 含 trace_id 關聯
2. 抽出共用 telemetry crate，避免 api 和 bo-api 各自維護一份 init/log_format/metrics 程式
3. bo-api handler + 關鍵 service-level 流程補 `#[tracing::instrument]`

## 約束

- **只做觀測性 parity，不改任何 business logic**：不動 handler 行為、不改 response 格式、不改 DB/Redis 邏輯
- **base metrics 共用，app-specific metrics 各自疊加**：`http_requests_total` + `http_request_duration_seconds` 放 telemetry crate；bo-api 未來若要加 admin-specific metrics（如 suspend 次數）由自己 crate 處理
- **api 既有行為不變**：api 改用 telemetry crate 時，只換 import，不改 runtime 行為
- **span 粒度中版**：handler 層 + service-level 子流程（如 `suspend_user` 的 DB / Redis / pubsub 各階段）補 span；不下沉到單一 repo method 或 cache 操作

## 背景

### 現況對比

| 項目 | api | bo-api |
|------|-----|--------|
| OTLP traces | ✅ | ❌ |
| Prometheus metrics + `/metrics` | ✅ | ❌ |
| JSON logs + trace_id 關聯 | ✅ | ❌ |
| Handler `#[instrument]` | 11 個檔案 | 只 auth 2 個 fn |
| `OTEL_EXPORTER_OTLP_ENDPOINT` config | ✅ | ❌ |

現在 bo-api 的 `init_tracing()` 只掛 `tracing_subscriber::fmt::layer()`，沒有 OTLP、沒有 metrics、logs 是 plain text。在 Grafana 上完全看不到 admin 流量的 trace 和指標。

### 為什麼要抽 crate

api 的 `log_format.rs`（SpanFieldsLayer + JsonWithTraceId + JsonVisitor）和 `init_telemetry()`（~30 行 OTel + Prometheus setup）都是**完全通用**的基礎建設。複製到 bo-api 後未來要改兩份。抽 crate 讓 base metrics middleware 也共用，但留空間給各 app 疊加自己的 metrics。

## 設計

### 新 crate：`crates/telemetry/`

```
crates/telemetry/
├── Cargo.toml
└── src/
    ├── lib.rs           # init_telemetry(otel_endpoint, service_name) -> PrometheusHandle
    ├── log_format.rs    # SpanFieldsLayer, JsonWithTraceId, JsonVisitor（從 api 搬過來）
    └── metrics.rs       # base_http_metrics middleware
```

**對外 API：**

```rust
pub fn init_telemetry(
    otel_endpoint: &str,
    service_name: &'static str,
) -> Result<PrometheusHandle, TelemetryInitError>;

pub use metrics::base_http_metrics;
```

**base metrics**：`http_requests_total` counter + `http_request_duration_seconds` histogram，labels `method / path / status`，與 api 現行完全一致。

**未來擴充**：如果 bo-api 需要 admin-specific metrics（例如 `admin_suspend_total`），由 bo-api 自己寫 middleware 或在 handler 內 `counter!()` 直接記，疊加在 base 之上，不進 telemetry crate。

### Span 粒度（中版）

**Handler 層**（一定要）：
- 所有 bo-api handler 都加 `#[tracing::instrument(skip(state), fields(...))]`
- dashboard / users (list / role / suspend / unsuspend) / streams (list / detail / force_end) / moderation (bans / chat)
- auth (login / refresh)

**Service-level 子流程**（中版重點）：
- `force_end_stream`：validate → kick MediaMTX → invalidate token → mark ended → pubsub notify（每階段一個 `info_span!` 或獨立 fn 帶 `#[instrument]`）
- `suspend_user`：update DB → write Redis access-state cache → pubsub broadcast（同上）
- `list_bans`（cross-broadcaster 聚合）：scan Redis keys → resolve user info → aggregate（子 span 標記聚合階段）

**不補**：單純的 repo method（如 `UserRepo::find_by_id`）、單一 cache 操作（如 `cache.set()`）——這些靠 parent span 的 field + log 就夠了。

## 影響範圍

### 新增

- `crates/telemetry/` — 新 crate
- `crates/telemetry/Cargo.toml`
- `crates/telemetry/src/lib.rs`（re-export `PrometheusHandle` 給外部用）
- `crates/telemetry/src/log_format.rs`
- `crates/telemetry/src/metrics.rs`
- `crates/bo-api/src/handlers/metrics.rs` — `/metrics` handler

### 修改

- `Cargo.toml`（workspace members + 把 telemetry deps 保留在 workspace.dependencies）
- `crates/api/Cargo.toml` — 加 telemetry crate 依賴（OTel/metrics 依賴透過 telemetry crate 傳遞，不需直接依賴）
- `crates/api/src/initializer.rs` — `init_telemetry` 改 call telemetry crate
- `crates/api/src/log_format.rs` — **刪除**（改用 telemetry crate）
- `crates/api/src/middleware/metrics.rs` — **刪除 base 部分**（改用 telemetry crate）
- `crates/bo-api/Cargo.toml` — 只加 `telemetry` crate 依賴；OTel 與 metrics 相關 crate 不直接依賴（由 telemetry crate 統一管理）
- `crates/bo-api/src/config.rs` — 加 `OTEL_EXPORTER_OTLP_ENDPOINT` 欄位
- `crates/bo-api/src/main.rs` — 改用 `telemetry::init_telemetry("streamhub-bo-api", endpoint)`
- `crates/bo-api/src/state.rs` — `BoAppState` 加 `metrics: PrometheusHandle`
- `crates/bo-api/src/routes.rs` — 加 `/metrics` route + 掛 base_http_metrics middleware
- `crates/bo-api/src/handlers/*.rs` — 補 `#[tracing::instrument]`
- `deploy/bo/.env.example` — 加 `OTEL_EXPORTER_OTLP_ENDPOINT`

### 不改

- Entity / DB schema
- 任何 handler 業務邏輯
- Response 格式
- api 的 runtime 行為（只換 import）
- nginx config
- 前端

如有異動，同步更新：
- [x] docs/architecture.md（telemetry crate + bo-api 觀測性說明）
- [x] docs/deploy.md（bo-api OTLP endpoint env var）

## Todo list

- [x] SPEC-035-01 建 `crates/telemetry/` crate —— `init_telemetry(endpoint, service_name)`、`log_format`（SpanFieldsLayer / JsonWithTraceId / JsonVisitor 從 api 搬過來）、`base_http_metrics` middleware、re-export `PrometheusHandle`
- [x] SPEC-035-02 api crate 改用 telemetry crate —— 刪 `crates/api/src/log_format.rs`、把 `init_telemetry` 內容替換成 `telemetry::init_telemetry(..., "streamhub-api")`，middleware 換成 `telemetry::base_http_metrics`。**驗證 api 行為完全不變**（既有測試必須通過）
- [x] SPEC-035-03 bo-api Cargo.toml 只加 `telemetry` crate 依賴（OTel/metrics crate 不直接依賴）
- [x] SPEC-035-04 bo-api config 加 `OTEL_EXPORTER_OTLP_ENDPOINT` 欄位（default `http://localhost:4317`），同步 `deploy/bo/.env.example`
- [x] SPEC-035-05 bo-api main.rs 改用 `telemetry::init_telemetry(&config.otel_endpoint, "streamhub-bo-api")`，取代現有 `init_tracing()`
- [x] SPEC-035-06 `BoAppState` 加 `metrics: PrometheusHandle`（從 telemetry crate re-export 取得）
- [x] SPEC-035-07 bo-api router 掛 `telemetry::base_http_metrics` middleware + 新增 `GET /metrics` route。**`/metrics` 是 Prometheus scrape endpoint，不掛 auth、不經 JWT 驗證、不套 admin rate limit**（確保實作時不會順手加到 admin auth 保護裡）
- [x] SPEC-035-08 bo-api handler 補 `#[tracing::instrument(skip(state), fields(...))]`，明確函式清單：
    - `handlers/dashboard.rs` — `dashboard`
    - `handlers/users.rs` — `list_users`、`update_role`、`suspend`、`unsuspend`
    - `handlers/streams.rs` — `list_streams`、`stream_detail`、`force_end`
    - `handlers/moderation.rs` — `list_bans`、`stream_chat`
    - `handlers/auth.rs` — `login`、`refresh`
- [x] SPEC-035-09 service-level 子流程 span（中版）—— 針對以下關鍵流程加子 span：`force_end`（validate / kick_mtx / invalidate_token / mark_ended / pubsub）、`suspend`（update_db / set_cache / broadcast）、`list_bans`（scan / resolve / aggregate）。**實作策略**：若子流程可自然抽成私有 helper fn，優先抽出並加 `#[instrument]`；若不值得抽函式，直接在 handler 內用 `tracing::info_span!("phase_name")` 包階段。不得為了加 span 重構 business logic
- [x] SPEC-035-10 測試與驗證：
    - **自動化測試**：新增 `crates/bo-api/src/tests/metrics_test.rs`，驗證 `GET /metrics` 回 200 + body 含 `http_requests_total` 或 `http_request_duration_seconds`
    - **人工 smoke**：啟動 bo-api → 打 `/v1/admin/dashboard` + `/v1/admin/users/:id/suspend` → Tempo 看到 `streamhub-bo-api` 的 trace（含子 span）；Grafana Loki 看到 JSON logs 含 `trace_id`
- [x] SPEC-035-11 更新 docs：architecture.md 加 telemetry crate 說明 + bo-api 觀測性；deploy.md 加 bo-api OTLP env var

## 驗收標準

- [x] `crates/telemetry/` crate 存在，api 和 bo-api 都依賴它
- [x] `crates/api/src/log_format.rs` 已刪除（改用 telemetry crate）
- [x] api 行為不變（既有所有測試通過，`/metrics` 還是回正確格式，service name 還是 `streamhub-api`）
- [ ] bo-api 啟動時連上 OTLP endpoint（log 可見 `init_telemetry` 成功）（人工 smoke）
- [ ] Tempo UI 能看到 `streamhub-bo-api` service，trace 帶 handler span + service-level 子 span（人工 smoke）
- [ ] Grafana 能 query bo-api 的 `http_requests_total{job="bo-api"}` 指標（人工 smoke）
- [ ] bo-api logs 是 JSON 格式，含 `trace_id` 欄位，能在 Grafana Loki 用 trace_id 跳到 Tempo（人工 smoke）
- [x] bo-api 所有 HTTP handler 都有 `#[tracing::instrument]`
- [x] `force_end_stream` / `suspend_user` / `list_bans` 在 Tempo 看到子 span（已抽 helper fn + `#[instrument]`，人工 smoke 驗證可視化）
- [x] CI 全綠（cargo build / test / clippy / fmt）

## 約束條件

- [x] telemetry crate 不依賴 api 或 bo-api（單向依賴）
- [x] base metrics 放 telemetry crate；app-specific metrics 留在各 app
- [x] 只改觀測性程式，不動 business logic
- [x] api 遷移 telemetry crate 時既有測試全過，runtime 行為不變
- [x] bo-api handler instrument 時 `skip(state)` + `skip(payload)` 避免大物件進 span attribute
- [x] 新 env var 同步 `.env.example`
- [ ] PR flow + Co-Authored-By

## 備註

- `log_format` 的 `JsonWithTraceId` 從 OpenTelemetry span context 取 trace_id，Loki 和 Tempo 自動關聯。這個能力對 bo-api 特別重要，admin 動作（suspend / force_end）需要能從 log 跳到 trace 看完整流程
- Prometheus recorder 是 global，一個 process 只能 install 一次——telemetry crate 要確保 `init_telemetry` 只被呼叫一次（否則 panic）。api 和 bo-api 是獨立 binary，各自呼叫自己的 `init_telemetry` 不會衝突
- 未來若要加 bo-api-specific metrics（如 `admin_suspend_total`、`admin_force_end_total`），直接在 handler 裡 `counter!()` 即可，Prometheus recorder 是 global
- Service name 會影響 Grafana 的服務選擇器，`streamhub-api` 和 `streamhub-bo-api` 分開，方便各自 dashboard
