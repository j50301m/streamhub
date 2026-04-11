# SPEC-013 GitHub CI

狀態：review

## 目標

建立 GitHub Actions CI pipeline，在每個 PR 與 push to main 時自動跑格式檢查、lint、測試、build 與 Docker build，確保 main 分支永遠綠燈。Dependabot 自動更新 Cargo 與 GitHub Actions 依賴。

**範圍界定：只做 CI（驗證），不做 CD（部署）**。部署留給 SPEC-011 GCE deployment。

## 影響範圍

新增：
- `.github/workflows/ci.yml` — 主 CI workflow
- `.github/dependabot.yml` — Cargo + GitHub Actions 依賴自動更新
- `rust-toolchain.toml` — pin toolchain channel（stable，跟最新穩定版）
- `justfile` — 本地 dev workflow 統一入口，指令與 CI 一致

修改：
- `Cargo.toml` — workspace 加 `rust-version = "1.85"`（MSRV 宣告）

文件同步：
- [ ] docs/architecture.md — 不影響
- [ ] CLAUDE.md — 不需要更新

## 架構設計

### Workflow trigger

```yaml
on:
  pull_request:
    branches: [main]
  push:
    branches: [main]
```

### Job 拆分（平行跑）

| Job | 內容 | 預期時間 |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | < 30s |
| `clippy` | `cargo clippy --all-targets --all-features -- -D warnings` | 3~5 min（首次）|
| `test` | `cargo test --all-features` | 3~5 min |
| `build` | `cargo build --release` | 5~8 min |
| `docker` | `docker build` API + web Dockerfile（不 push）| 5~10 min |

所有 job 平行獨立，靠 cache 加速重跑。

### Cache 策略

使用 `Swatinem/rust-cache@v2`：
- cargo registry / git
- `target/` 目錄
- Cache key 依 `Cargo.lock` hash + job 名稱分流

Docker job 用 `docker/build-push-action@v6` 的 GitHub Actions cache（`cache-from: type=gha`、`cache-to: type=gha,mode=max`）。

### Concurrency

```yaml
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true
```

PR push 新 commit 時自動取消前一次 in-flight run，省 CI quota。

### Rust toolchain

`rust-toolchain.toml` 用 `channel = "stable"`（跟最新 stable），`components = ["rustfmt", "clippy"]`。workspace `Cargo.toml` 加 `rust-version = "1.85"` 作為 MSRV 宣告。

### Dependabot

每週檢查：
- `package-ecosystem: cargo`（workspace 根）
- `package-ecosystem: github-actions`（`.github/workflows/`）

自動開 PR，人工審核後 merge。限制同時最多 5 個 open PR。

## Todo list

- [x] SPEC-013-01 `rust-toolchain.toml` — `channel = "stable"`、`components = ["rustfmt", "clippy"]`
- [x] SPEC-013-02 workspace `Cargo.toml` — 加 `[workspace.package]` 的 `rust-version = "1.85"`（或在 `[workspace]` 下適當位置），確認 `cargo build` 仍成功
- [x] SPEC-013-03 `.github/workflows/ci.yml` — trigger、concurrency、5 個 job（fmt / clippy / test / build / docker）
- [x] SPEC-013-04 Rust cache — 用 `Swatinem/rust-cache@v2`（fmt / clippy / test / build 共用一種 cache key 策略）
- [x] SPEC-013-05 Docker build job — `docker/build-push-action@v6`，build 兩個 Dockerfile（`deploy/services/Dockerfile.api`、`Dockerfile.web`），不 push，用 gha cache
- [x] SPEC-013-06 `.github/dependabot.yml` — cargo + github-actions 每週 schedule
- [x] SPEC-013-07 本地驗證 — `cargo build` + `cargo clippy -- -D warnings` + `cargo fmt --check` + `cargo test` 全部綠燈（避免 push 上去才發現錯）
- [x] SPEC-013-08 justfile — 本地 `just check` / `just check-docker`，指令與 CI 一致

## 驗收流程

### 本地驗證（implementer 必跑）

```bash
cd worktrees/feat-spec-013
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release

# 也要確認 Dockerfile 還能 build
docker build -f deploy/services/Dockerfile.api -t streamhub-api:ci .
docker build -f deploy/services/Dockerfile.web -t streamhub-web:ci .
```

### Push 後觀察

```bash
git push origin feat/spec-013-github-ci
# 在 GitHub PR 頁面開啟 PR（base: main, compare: feat/spec-013-github-ci）
```

預期 GitHub Actions 顯示：
- ✅ `fmt`
- ✅ `clippy`
- ✅ `test`
- ✅ `build`
- ✅ `docker`

全綠後才能通知 reviewer。

### 失敗情境驗證（實作完成後補測一次）

故意 push 一個 unformatted 或有 warning 的 commit 到 branch，確認 CI 紅燈擋下。驗證完 revert。

### Branch protection（由使用者手動設定，spec 外）

部署完成後使用者到 GitHub 設定：
1. Settings → Branches → Add rule → Branch name pattern: `main`
2. Require status checks: `fmt` / `clippy` / `test` / `build` / `docker`
3. Require branches to be up to date before merging
4. Require pull request reviews before merging（可選）

此步驟非 spec 內 todo，但 spec 完成後會列在交付通知中提醒使用者。

## 備註

- **為什麼不跑 DB 測試**：所有現有 test 都用 `sea_orm::MockDatabase`，不需要 postgres service container（已確認 `crates/api/src/tests/*.rs` 皆用 mock）
- **為什麼不包 CD**：部署 scope 大（GCP auth、image registry、rolling update、secret management），留 SPEC-011 / 獨立 spec 處理
- **為什麼包 Docker build**：Dockerfile 依賴（Rust builder image、ffmpeg、glibc）踩過坑（memory `project_observability_gotchas` 有記），CI 持續驗證可早點發現 runtime image 壞掉
- **Cost 考量**：GitHub-hosted runner（ubuntu-latest）免費額度足夠。未來量大再評估 self-hosted
- **不做 code coverage**：暫不引入 tarpaulin/llvm-cov（scope 控制），後續 spec 再加
- **不做 release job**：沒有版本發布需求
- **Dependabot open PR 限制**：cargo open-pull-requests-limit 5、github-actions limit 3，避免一次開太多
- **rust-toolchain stable**：跟最新 stable，Rust 新 release 可能偶爾紅燈，屆時修 lint 即可；若頻率太高再改成 pin 特定版本
- **justfile**：本地開發用 `just check` 跑 CI 同組檢查（fmt/clippy/test/build），`just check-docker` 驗證 Dockerfile；依賴 `brew install just`。不做 git pre-push hook，因為 GitHub Actions + branch protection 已經擋下 push，hook 只是重複驗證且會拖慢 commit 流程
