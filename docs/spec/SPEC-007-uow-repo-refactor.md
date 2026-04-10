# SPEC-007 Repository + Unit of Work 重構

狀態：done

## 目標

將所有 DB 操作從 route handler 抽離，改為透過 Repository trait + Unit of Work pattern。
寫入操作使用 transaction，需要時用 SELECT FOR UPDATE 鎖定 record。
AppState 改為持有 AppConfig 而非展開的個別欄位。

## 影響範圍

新增：
- `crates/repo/` — UnitOfWork、TransactionContext、RepoProvider、各 Repo trait + 實作

修改：
- `crates/common/src/lib.rs` — AppState 改為持有 UnitOfWork + AppConfig
- `crates/api/src/routes/auth.rs` — 改用 repo
- `crates/api/src/routes/streams.rs` — 改用 repo
- `crates/hook/src/publish.rs` — 改用 repo
- `crates/hook/src/recording.rs` — 改用 repo
- `crates/hook/src/mediamtx_auth.rs` — 改用 repo
- `crates/api/src/main.rs` — AppState 初始化
- `crates/api/src/middleware/auth.rs` — CurrentUser 改用 repo

## Todo list

- [x] SPEC-007-01 repo crate 基礎架構 — UnitOfWork、TransactionContext、RepoProvider trait、ConnectionTrait 抽象
- [x] SPEC-007-02 StreamRepo trait + 實作 — find_by_id、find_by_key、find_by_key_for_update、list_live、list_vod、list_by_user(paginated)、create、update、delete
- [x] SPEC-007-03 UserRepo trait + 實作 — find_by_id、find_by_email、find_by_email_for_update、create
- [x] SPEC-007-04 RecordingRepo trait + 實作 — create、list_by_stream(paginated)、find_latest_by_stream
- [x] SPEC-007-05 StreamTokenRepo trait + 實作 — create、find_by_stream_and_hash
- [x] SPEC-007-06 AppState 重構 — 持有 UnitOfWork + AppConfig，移除展開的欄位
- [x] SPEC-007-07 重構 routes/auth.rs — register 用 txn（for_update 防並發）、login/refresh/me 用 uow
- [x] SPEC-007-08 重構 routes/streams.rs — create/update/end/delete/token 用 txn + for_update、list/get 用 uow
- [x] SPEC-007-09 重構 hook/ — publish_hook 用 txn + for_update、recording_hook 用 txn + for_update（含 transcode trigger）
- [x] SPEC-007-10 重構 middleware/auth.rs + hook/mediamtx_auth.rs — 改用 repo
- [x] SPEC-007-11 驗證 — cargo build + clippy + fmt

## 架構設計

### UnitOfWork + TransactionContext

```rust
/// 非交易式存取，用於讀取操作
pub struct UnitOfWork {
    db: DatabaseConnection,
}

impl UnitOfWork {
    pub fn new(db: DatabaseConnection) -> Self { Self { db } }
    pub async fn begin(&self) -> Result<TransactionContext> {
        let txn = self.db.begin().await?;
        Ok(TransactionContext { txn: Some(txn) })
    }
}

/// 交易式存取，用於寫入操作
pub struct TransactionContext {
    txn: Option<DatabaseTransaction>,
}

impl TransactionContext {
    pub async fn commit(mut self) -> Result<()> { ... }
    pub async fn rollback(mut self) -> Result<()> { ... }
}
```

### RepoProvider trait

```rust
pub trait RepoProvider {
    fn stream_repo(&self) -> &dyn StreamRepo;
    fn user_repo(&self) -> &dyn UserRepo;
    fn recording_repo(&self) -> &dyn RecordingRepo;
    fn stream_token_repo(&self) -> &dyn StreamTokenRepo;
}

// UnitOfWork 和 TransactionContext 都實作 RepoProvider
impl RepoProvider for UnitOfWork { ... }
impl RepoProvider for TransactionContext { ... }
```

### Repo trait 範例

```rust
#[async_trait]
pub trait StreamRepo {
    async fn find_by_id(&self, id: Uuid) -> Result<Option<stream::Model>>;
    async fn find_by_key(&self, key: &str) -> Result<Option<stream::Model>>;
    async fn find_by_key_for_update(&self, key: &str) -> Result<Option<stream::Model>>;
    async fn list_live(&self) -> Result<Vec<stream::Model>>;
    async fn create(&self, model: stream::ActiveModel) -> Result<stream::Model>;
    async fn update(&self, model: stream::ActiveModel) -> Result<stream::Model>;
    // ...
}
```

### FOR UPDATE 使用場景

| 場景 | 方法 | 理由 |
|------|------|------|
| register | find_by_email_for_update | 防止同 email 並發註冊 |
| publish/unpublish hook | find_by_key_for_update | 防止同 stream 並發狀態更新 |
| create stream token | find_by_id_for_update (stream) | owner 檢查 + token 寫入原子 |
| recording hook | find_by_key_for_update (stream) | recording 寫入 + vod_status 更新原子 |
| update/end/delete stream | find_by_id_for_update | 防止並發修改 |

### AppState 改動

```rust
// Before
pub struct AppState {
    pub db: DatabaseConnection,
    pub mediamtx_url: String,
    pub jwt_secret: String,
    pub recordings_path: String,
}

// After
pub struct AppState {
    pub uow: UnitOfWork,
    pub config: AppConfig,
}
```

handler 存取方式：
- DB：`state.uow.stream_repo().find_by_id(id)` 或 `state.uow.begin().await?`
- Config：`state.config.jwt_secret`、`state.config.recordings_path`

### Handler 使用範例

```rust
// 讀取
async fn get_stream(State(state): State<AppState>, ...) {
    let stream = state.uow.stream_repo().find_by_id(id).await?;
}

// 寫入（txn + for_update）
async fn publish_hook(State(state): State<AppState>, ...) {
    let txn = state.uow.begin().await?;
    let stream = txn.stream_repo().find_by_key_for_update(&key).await?
        .ok_or(AppError::NotFound)?;
    txn.stream_repo().update(active_model).await?;
    txn.commit().await?;
}
```

## 驗收標準

- 所有 route handler 和 hook handler 不直接呼叫 SeaORM Entity
- 寫入操作都用 TransactionContext
- 需要防並發的場景用 FOR UPDATE
- AppState 只有 uow + config
- cargo build + clippy + fmt 全過
- 功能行為不變

## 備註

- repo crate 依賴 entity crate 和 sea-orm
- async_trait 用於 repo trait（因為 async fn in trait 需要）
- FOR UPDATE 透過 SeaORM 的 `lock(LockType::Update)` 實作
- UnitOfWork 需要 Clone（因為 AppState 需要 Clone for Axum）
- TransactionContext 不需要 Clone（用完即 commit/rollback）
