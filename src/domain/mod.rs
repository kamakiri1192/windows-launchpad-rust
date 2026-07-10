//! Durable launcher data and pure rules.
//!
//! The domain layer holds stable launcher concepts that must persist or affect
//! launcher identity: app IDs, the app registry, settings, and app-list diffs.
//! These modules are pure data — they depend only on `std`, `serde`, and other
//! library-layer types ([`crate::ui_model`]). They must not depend on `winit`,
//! `wgpu`, Win32, the app shell, or any feature/renderer/worker module.
//!
//! See `ARCHITECTURE.md > Domain Model` for the target shape. Phase 7 will
//! extend this layer with `LauncherItem`, `FolderId`, and `Folder`; Phase 6.5
//! only relocates the existing `AppId`, `AppRegistry`, `Settings`, and
//! `AppDiff` here so the dependency direction is explicit.

pub mod app_diff;
pub mod app_id;
pub mod app_registry;
pub mod settings;
