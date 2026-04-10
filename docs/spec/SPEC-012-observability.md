# SPEC-012 監控與可觀測性

狀態：in-progress

## 目標

加入結構化日誌、分散式追蹤、基礎指標收集，讓服務運行狀態可觀測。
本地開發可透過 Grafana dashboard 查看。
業務指標留後續 spec。

## 三大支柱

| 支柱 | 工具 | 用途 |
|------|------|------|
| **Logs** | tracing + OpenTelemetry → Loki | 結構化日誌，按 trace_id / stream_id 查詢 |
| **Traces** | OpenTelemetry → Tempo | 追蹤 request 生命週期（API → DB → GCS） |
| **Metrics** | Prometheus | 基礎指標：HTTP request count/latency/status、CPU、memory |

## 影響範圍

修改：
- `crates/api/src/main.rs` — 初始化 OpenTelemetry provider、Prometheus exporter
- `crates/common/src/config.rs` — OTEL_EXPORTER_OTLP_ENDPOINT
- `Cargo.toml` — OpenTelemetry 相關 dependency

新增：
- `crates/api/src/middleware/metrics.rs` — Prometheus HTTP metrics middleware
- `deploy/docker-compose.observability.yml` — 獨立 compose：Grafana + Prometheus + Tempo + Loki
- `deploy/prometheus.yml` — Prometheus scrape config
- `deploy/tempo.yml` — Tempo config
- `deploy/loki.yml` — Loki config
- `deploy/grafana/` — datasource provisioning + dashboard JSON

## Todo list

- [x] SPEC-012-01 OpenTelemetry tracing — OTLP exporter → Tempo，request span 自動注入 trace_id
- [x] SPEC-012-02 structured logging — tracing log 輸出到 stdout（JSON 格式）+ 推送到 Loki，帶 trace_id context
- [x] SPEC-012-03 Prometheus HTTP metrics — GET /metrics endpoint，暴露 request duration/count/status
- [x] SPEC-012-04 Config — OTEL_EXPORTER_OTLP_ENDPOINT
- [x] SPEC-012-05 docker-compose.observability.yml — Prometheus(9090) + Tempo(4317) + Loki(3100) + Grafana(3001)
- [x] SPEC-012-06 Prometheus config — scrape API :8080/metrics + node-exporter（CPU/memory）
- [x] SPEC-012-07 node-exporter — 容器內 CPU/memory 指標（或用 cAdvisor）
- [x] SPEC-012-08 Tempo + Loki config — 接收 OTLP traces、接收 log
- [x] SPEC-012-09 Grafana provisioning — datasource（Prometheus + Tempo + Loki）+ API overview dashboard
- [x] SPEC-012-10 Grafana dashboard — HTTP RPS、latency p50/p95/p99、error rate、CPU、memory
- [x] SPEC-012-11 驗證 — cargo build + test + clippy，Grafana dashboard 可查看

## 架構設計

### 資料流

```
API server
  ├── tracing spans ──→ OTLP gRPC ──→ Tempo (4317)
  ├── structured logs ──→ Loki (3100)
  └── /metrics ──→ Prometheus scrape (9090)

node-exporter / cAdvisor
  └── CPU/memory metrics ──→ Prometheus scrape

Grafana (3001)
  ├── Prometheus datasource → Metrics dashboard
  ├── Tempo datasource → Trace 查詢
  └── Loki datasource → Log 查詢（可用 trace_id 關聯）
```

### Observability Stack（獨立 compose）

```bash
# 啟動方式
docker compose -f docker-compose.yml -f docker-compose.observability.yml up -d

# 只啟動服務（不要監控）
docker compose up -d
```

```yaml
# deploy/docker-compose.observability.yml
services:
  prometheus:
    image: prom/prometheus
    container_name: streamhub-prometheus
    ports: ["9090:9090"]
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro

  tempo:
    image: grafana/tempo
    container_name: streamhub-tempo
    ports: ["4317:4317"]
    command: ["-config.file=/etc/tempo.yml"]
    volumes:
      - ./tempo.yml:/etc/tempo.yml:ro

  loki:
    image: grafana/loki
    container_name: streamhub-loki
    ports: ["3100:3100"]
    command: ["-config.file=/etc/loki.yml"]
    volumes:
      - ./loki.yml:/etc/loki.yml:ro

  node-exporter:
    image: prom/node-exporter
    container_name: streamhub-node-exporter
    ports: ["9100:9100"]

  grafana:
    image: grafana/grafana
    container_name: streamhub-grafana
    ports: ["3001:3000"]
    environment:
      GF_AUTH_ANONYMOUS_ENABLED: "true"
      GF_AUTH_ANONYMOUS_ORG_ROLE: Admin
    volumes:
      - ./grafana/provisioning:/etc/grafana/provisioning:ro
      - ./grafana/dashboards:/var/lib/grafana/dashboards:ro
```

### API 側改動

```rust
// main.rs 初始化
// 1. OpenTelemetry tracer → Tempo
let tracer = opentelemetry_otlp::new_pipeline()
    .tracing()
    .with_exporter(opentelemetry_otlp::new_exporter().tonic())
    .install_batch(opentelemetry_sdk::runtime::Tokio)?;

// 2. tracing subscriber 加 OpenTelemetry layer
tracing_subscriber::registry()
    .with(tracing_opentelemetry::layer().with_tracer(tracer))
    .with(tracing_subscriber::fmt::layer().json())  // JSON structured log
    .with(EnvFilter::...)
    .init();

// 3. Prometheus metrics middleware
let metrics_handle = axum_prometheus::PrometheusMetricLayer::pair();
app.layer(metrics_handle.0)
   .route("/metrics", get(|| async move { metrics_handle.1.render() }))
```

### Grafana Dashboard 內容

| Panel | 資料來源 | 說明 |
|-------|---------|------|
| HTTP RPS | Prometheus | `rate(http_requests_total[5m])` |
| Latency p50/p95/p99 | Prometheus | `histogram_quantile(0.95, ...)` |
| Error Rate | Prometheus | `rate(http_requests_total{status=~"5.."}[5m])` |
| CPU Usage | Prometheus (node-exporter) | `node_cpu_seconds_total` |
| Memory Usage | Prometheus (node-exporter) | `node_memory_MemAvailable_bytes` |
| Recent Traces | Tempo | 最近的 request traces |
| Log Stream | Loki | 即時 log 查詢 |

## 備註

- SeaORM 2.0 已啟用 tracing feature，DB query 自動有 span
- OpenTelemetry 不影響現有 `tracing::info!` 等 macro，只是額外 export
- Grafana 用 port 3001 避免跟 nginx 的 3000 衝突
- node-exporter 在 Docker 環境下看到的是 host 的 CPU/memory
- 業務指標（active_streams、transcode_duration 等）留後續 spec
- Log 推送到 Loki 可用 Promtail（sidecar）或直接從 Docker log driver 送
