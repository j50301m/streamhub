# SPEC-008 API 結構重整

狀態：done

## 目標

將 hook crate 合併進 api crate，route binding 和 handler 分離，test 獨立檔案。

## Todo list

- [x] SPEC-008-01 建立 handlers/ 目錄，搬移 auth、streams handler 邏輯（不含 route binding）
- [x] SPEC-008-02 搬移 hook handlers（publish、recording、mediamtx_auth）到 handlers/
- [x] SPEC-008-03 routes.rs 統一管理所有 route binding
- [x] SPEC-008-04 tests/ 目錄，搬移所有 unit test 到獨立檔案
- [x] SPEC-008-05 刪除 crates/hook/，從 workspace 移除，更新所有依賴
- [x] SPEC-008-06 驗證 — cargo build + test + clippy + fmt
