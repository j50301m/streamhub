//! SeaORM entity definitions for the streamhub database (`users`, `streams`,
//! `recordings`). Each module defines the table model, column enum, and
//! status enums used across repos and handlers.
//!
//! Fields mirror database columns one-to-one; per-field rustdoc is
//! intentionally omitted (see SPEC-024) — the authoritative schema reference
//! is the migration / entity source itself.
#![allow(missing_docs)]

pub mod recording;
pub mod stream;
pub mod user;
