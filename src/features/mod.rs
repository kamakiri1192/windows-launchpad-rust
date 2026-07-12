//! Feature modules: user-facing behavior and per-feature state.
//!
//! Each feature owns its own state, update logic, and feature-specific intent.
//! Feature modules decide **what the UI means**; they do not directly own
//! windows, GPU resources, Windows handles, or the global event loop. Side
//! effects are requested through narrow command/outcome types and executed by
//! the app boundary (`main.rs`).
//!
//! Phase 4 only introduces [`edit_mode`]; other features (search, folders,
//! settings, bottom_control, icons, app_list) are added in later phases per
//! `docs/DF_REARCHITECTURE_PLAN.md`.

pub mod bottom_control;
pub mod edit_mode;
