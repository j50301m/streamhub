# SPEC-036 Trace Context Propagation（bo-api ↔ api）

狀態：done

## 目標

把 bo-api 和 api 的 trace 串起來。admin force_end / suspend 等跨服務動作在 Tempo 要看到一條連續 trace，涵蓋 bo-api handler → Redis pubsub → api subscriber → MediaMTX kick 的完整流程。

涵蓋範圍：
1. **Redis pubsub**：bo-api publish → api subscribe 的 trace 繼承（核心需求）
2. **Inbound HTTP**：api + bo-api 的 TraceLayer 從 incoming `traceparent` header 繼承 parent context（未來 MediaMTX 或 frontend 若送會生效）

**不包含**（留給未來 spec）：
- Outbound HTTP propagation（reqwest middleware 注入）— 需要 `reqwest-middleware` 依賴且 GCP 側不會回傳 trace 資料，投資報酬低

## 約束

- **向後相容**：pubsub payload 的 `traceparent` 欄位必須是 `Option<String>`。舊 publisher 不送或送 malformed 時，subscriber 要能正常運作（建新 root span，不要 panic）
- **不動 business logic**：只加 trace context 的 inject / extract，不改 pubsub channel、payload 主要欄位、handler 邏輯
- **telemetry crate 維持單向依賴**：inject / extract helper 放 telemetry crate，bo-api / api 呼叫 helper，不讓各 crate 重複寫 propagator 程式
- **api 既有 OTel 行為不變**：既有 handler / subscriber span 結構不變，只是多了 parent context 連接
- **不引入新的外部依賴**：`opentelemetry 0.31` 已提供 `TraceContextPropagator`，tower-http 0.6 的 `TraceLayer` 支援 `make_span_with` 自訂，不需要 `opentelemetry-http` 或 `reqwest-middleware`

## 背景

### 現況

`crates/telemetry/src/lib.rs` 沒設定 text map propagator，所以：
- Redis pubsub publish 不會帶 trace 資訊
- Subscriber 收到訊息時建立的 span 是 orphan root span，跟 publisher 的 trace 無關
- Inbound HTTP `traceparent` header（若有）不會被 TraceLayer 讀取

結果：admin 在 bo-api 按 force_end，在 Tempo 看到 bo-api 的 trace 只到 pubsub publish 那行就斷了；api 側的 `handle_admin_force_end` 是另一條獨立 trace。中間看不出關聯，debug 跨服務問題要手動拼 timestamp。

### Pubsub 呼叫點盤點

**bo-api → api**（使用者核心關心）：
- `streamhub:admin_force_end`：bo-api `handlers/streams.rs:352` → api `redis_subscriber.rs:96`
- `streamhub:user_suspended`：bo-api `handlers/users.rs:228` → api `redis_subscriber.rs:56`

**api 內部（跨 tokio task）**：
- `streamhub:events`：api `redis_subscriber.rs:346` publish → api `redis_subscriber.rs:152` subscribe（live_streams / viewer_count / reconnect）
- `streamhub:chat:{id}`：api `handlers/chat.rs:213` publish → api `handlers/chat.rs:295+` subscribe（跨 instance chat 傳播）

雖然內部 pubsub 不是跨 service，但同 process 不同 tokio task 的 span 也會斷，順便一起做成本低，統一 inject/extract 機制。

## 設計

### 1. Telemetry crate 加 helpers

`crates/telemetry/src/lib.rs` 在 `init_telemetry` 裡加：

```rust
opentelemetry::global::set_text_map_propagator(
    opentelemetry_sdk::propagation::TraceContextPropagator::new(),
);
```

新增 `crates/telemetry/src/propagation.rs` 提供：

```rust
/// Serialize the current span's context as a W3C traceparent string.
/// Returns None if no active span or sampling disabled.
pub fn inject_traceparent() -> Option<String>;

/// Parse a W3C traceparent string and build an OpenTelemetry Context.
/// Returns None if input is None, empty, or malformed (fail-soft).
pub fn extract_parent_context(traceparent: Option<&str>) -> Option<opentelemetry::Context>;

/// tower-http TraceLayer make_span closure — builds an HTTP span and sets
/// parent from the incoming traceparent header if present.
pub fn http_make_span(req: &axum::http::Request<...>) -> tracing::Span;

/// Inline HTTP header extractor (implements opentelemetry::propagation::Extractor)
/// — used by http_make_span, no external opentelemetry-http dep.
```

### 2. Pubsub payload 加 traceparent

**admin_force_end payload**（`crates/bo-api/src/handlers/streams.rs`）：

```rust
#[derive(Serialize, Deserialize)]
pub struct AdminForceEndPayload {
    pub stream_id: Uuid,
    pub requested_by: Uuid,
    pub requested_at: String,
    /// W3C traceparent from publisher's current span (SPEC-036)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}
```

**user_suspended payload**、**events（RedisEvent enum variants）**、**chat pubsub envelope**：同樣加 `Option<String>` 欄位。enum 結構若不便加共用欄位，可用 envelope struct 包裝：

```rust
#[derive(Serialize, Deserialize)]
struct TracedEnvelope<T> {
    #[serde(flatten)]
    payload: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    traceparent: Option<String>,
}
```

實作時選簡單的：單一 struct 直接加欄位；enum / 多型訊息用 envelope。由 implementer 判斷每個 case。

**chat 特別注意**：
- `traceparent` 是內部 pubsub carrier metadata，不是對外 WS schema 的一部分
- 不要把 `traceparent` 加進 `ChatMessagePayload` 這種 client-facing type，避免污染前端 contract
- `streamhub:chat:{id}` 同一條通道目前至少承載：
  - `ServerMessage::ChatMessage`（`handlers/chat.rs`）
  - `ServerMessage::ChatMessageDeleted`（`handlers/chat_moderation.rs`）
- 因此 chat 應使用 internal `TracedEnvelope<ServerMessage>` 或等價的 pubsub-only wrapper：publish 時包起來，subscribe 時先 extract traceparent，再把內層 `ServerMessage` 原樣轉發給 WS clients

### 3. Publisher inject / Subscriber extract

**Publisher**（publish 前呼叫）：

```rust
let payload = AdminForceEndPayload {
    stream_id,
    requested_by,
    requested_at,
    traceparent: telemetry::inject_traceparent(),
};
pubsub.publish(channel, &serde_json::to_string(&payload)?).await?;
```

**Subscriber**（收到後建 span 並 set_parent）：

```rust
use tracing::Instrument;
let parent_ctx = telemetry::extract_parent_context(payload.traceparent.as_deref());
let span = tracing::info_span!("handle_admin_force_end", stream_id = %payload.stream_id);
if let Some(ctx) = parent_ctx {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    span.set_parent(ctx);
}
handle_admin_force_end(...).instrument(span).await;
```

### 4. HTTP inbound extractor

api 和 bo-api 的 TraceLayer 改用 telemetry crate 提供的 `http_make_span`：

```rust
// 現況 (api initializer.rs:149):
TraceLayer::new_for_http()
    .make_span_with(DefaultMakeSpan::new().level(Level::INFO))

// 改為：
TraceLayer::new_for_http()
    .make_span_with(telemetry::http_make_span)
```

`http_make_span` 內部會：
1. 從 request headers 找 `traceparent`
2. 用 global propagator extract 成 Context
3. 建 span 並 set_parent

MediaMTX 目前不送 `traceparent`，所以短期無感；未來 frontend 或其他服務送的話就能接上。

## 影響範圍

### 新增

- `crates/telemetry/src/propagation.rs` — inject / extract helpers + HTTP extractor

### 修改

- `crates/telemetry/src/lib.rs` — 加 `set_text_map_propagator` 呼叫、re-export propagation helpers
- `crates/bo-api/src/handlers/streams.rs` — `AdminForceEndPayload` 加欄位 + publisher inject
- `crates/bo-api/src/handlers/users.rs` — `user_suspended` payload 加欄位 + publisher inject
- `crates/bo-api/src/main.rs` — TraceLayer 改用 `telemetry::http_make_span`
- `crates/api/src/tasks/redis_subscriber.rs` — 所有 subscriber 改從 payload extract + `set_parent`；內部 `streamhub:events` subscriber extract
- `crates/api/src/handlers/chat.rs` — chat pubsub publish inject + subscribe extract（用 internal traced envelope 包 `ServerMessage`，不改 WS 對外 payload）
- `crates/api/src/handlers/chat_moderation.rs` — chat delete publish inject（同 chat traced envelope）
- `crates/api/src/tasks/viewer_count.rs` — `streamhub:events` publish inject
- `crates/api/src/handlers/publish.rs` — `streamhub:events` publish inject（`LiveStreams`）
- `crates/api/src/handlers/drain.rs` — `streamhub:events` publish inject（`Reconnect`）
- `crates/api/src/initializer.rs` — TraceLayer 改用 `telemetry::http_make_span`
- `crates/api/src/ws/types.rs` — `RedisEvent` 若採 envelope / metadata carrier 需相應調整；**不得**為了 propagation 修改 client-facing `ChatMessagePayload`

### 不改

- Entity / DB schema
- Pubsub channel 名稱
- Handler business logic
- Response 格式
- 前端
- nginx config
- Outbound reqwest calls（留給未來 spec）
- MediaMTX webhook handlers 的 business logic（只靠 TraceLayer 自動繼承，不改 handler）

如有異動，同步更新：
- [ ] docs/architecture.md（觀測性章節加 trace propagation 說明）
- [ ] docs/deploy.md（若有新 env 需設定）

## Todo list

- [x] SPEC-036-01 `crates/telemetry/src/propagation.rs`：`inject_traceparent`、`extract_parent_context`、`http_make_span`、inline HeaderExtractor。`lib.rs` 在 `init_telemetry` 加 `set_text_map_propagator(TraceContextPropagator::new())` + re-export helpers
- [x] SPEC-036-02 bo-api publisher：`admin_force_end` payload 加 `traceparent: Option<String>`，publish 前呼叫 `telemetry::inject_traceparent()`
- [x] SPEC-036-03 bo-api publisher：`user_suspended` payload 同上
- [x] SPEC-036-04 api subscriber：`admin_force_end` / `user_suspended` 從 payload extract parent context，建 span 後 `set_parent`
- [x] SPEC-036-05 api 內部 pubsub：`streamhub:events`（`RedisEvent` enum，用 envelope 或直接加欄位）publisher inject + subscriber extract。**盤點所有 publisher 並覆蓋：**
  - `crates/api/src/tasks/viewer_count.rs` — `ViewerCount`
  - `crates/api/src/handlers/publish.rs` — `LiveStreams`
  - `crates/api/src/handlers/drain.rs` — `Reconnect`
- [x] SPEC-036-06 api 內部 pubsub：chat publish inject + subscribe extract。**要求**：使用 internal traced envelope 包 `ServerMessage`，至少覆蓋：
  - `crates/api/src/handlers/chat.rs` — `ServerMessage::ChatMessage`
  - `crates/api/src/handlers/chat_moderation.rs` — `ServerMessage::ChatMessageDeleted`
  - 不修改 client-facing `ChatMessagePayload` / `ServerMessage` HTTP/WS contract
- [x] SPEC-036-07 api + bo-api TraceLayer 改用 `telemetry::http_make_span`（取代 `DefaultMakeSpan`）
- [x] SPEC-036-08 Integration test：verify roundtrip —— 建 span → `inject_traceparent` → `extract_parent_context` → 建新 span `set_parent` → 新 span 的 trace_id 必須等於原 span 的 trace_id。另外加一個 pubsub 端到端 test（publisher → subscriber 同一 trace_id）
- [x] SPEC-036-09 向後相容 test：`extract_parent_context(None)` 回 None、malformed string 回 None、subscriber 收到沒有 traceparent 的 payload 不會 panic 且建新 root span
- [x] SPEC-036-10 HTTP inbound test：帶 `traceparent` header 的 request 經 `telemetry::http_make_span` 後，建立出的 span trace_id 與 header 對應 context 一致；沒有 / malformed header 時不 panic 且建立新 root span。**目的**：確保 Scope B 的 inbound HTTP propagation 有自動化驗證，不只靠人工 smoke
- [x] SPEC-036-11 更新 docs/architecture.md 觀測性章節：加 trace propagation 機制說明（propagator / pubsub carrier / HTTP inbound extractor），列 scope 限制（outbound HTTP 未做）

## 驗收標準

- [x] `cargo test` 全綠（含新 propagation integration tests）
- [x] telemetry crate 有 `propagation` module，對外 re-export `inject_traceparent` / `extract_parent_context` / `http_make_span`
- [x] bo-api publish `admin_force_end` 後，payload JSON 含 `traceparent` 欄位
- [x] api subscriber 收到有 traceparent 的 payload 後，新建 span 的 trace_id 與 publisher 一致（test 驗證）
- [x] 舊 payload（無 traceparent 欄位）不會 break subscriber
- [x] 帶 `traceparent` header 的 HTTP request 進 api / bo-api 時，root HTTP span 會繼承 incoming trace（test 驗證 `http_make_span`）
- [ ] 人工 smoke：admin 在 bo-api 按 force_end → Tempo 看到一條連續 trace 包含 bo-api handler span + api `handle_admin_force_end` span + 其子 span（人工驗收）
- [ ] 人工 smoke：suspend user → Tempo 看到 bo-api → api `disconnect_user` 連續 trace（人工驗收）
- [x] api + bo-api HTTP span 結構不變（既有 log / dashboard 還能用）
- [x] 沒有引入新的 workspace 依賴（opentelemetry 0.31 / tower-http 0.6 既有版本足夠）
- [x] CI 全綠（build / test / clippy / fmt）

## 約束條件

- [x] pubsub payload `traceparent` 一律 `Option<String>` + `#[serde(default, skip_serializing_if = "Option::is_none")]`
- [x] extract fail-soft：malformed traceparent 不 panic，log warn 後當作無 parent
- [x] telemetry crate 內 no unwrap/expect（test 除外）
- [x] HTTP TraceLayer 行為改變後既有 handler span 結構不變
- [x] `streamhub:events` 的所有現有 publisher 都已注入 propagation metadata，不可只改單一路徑
- [x] chat propagation 使用 pubsub-only wrapper，不修改 client-facing WS schema
- [x] 不改 pubsub channel 名稱、不改 pubsub business behavior
- [x] PR flow + Co-Authored-By

## 備註

- W3C Trace Context propagator 和 opentelemetry 0.31 是內建，不需額外 crate
- HeaderExtractor 實作放 telemetry crate inline（~15 行），不引入 `opentelemetry-http` 新依賴
- MediaMTX 是 Go binary，我們不能讓它送 `traceparent`。SPEC-036 的 HTTP inbound 改造是「能接就接，不強求」；MediaMTX webhook 的 trace 短期內仍會是 orphan root span
- 未來若要做 outbound HTTP propagation（api → MediaMTX / GCS / Transcoder），會是 SPEC-037 的 scope。需要加 `reqwest-middleware` 依賴並改所有 reqwest client 建構
- 這個 spec 完成後，「admin 動作」的跨服務可觀測性應該顯著提升；chat / events 等內部 pubsub 的 trace 也會更完整
