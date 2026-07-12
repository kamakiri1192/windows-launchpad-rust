//! Durable launcher data and pure rules.
//!
//! The domain layer holds stable launcher concepts that must persist or affect
//! launcher identity: app IDs, the app registry, settings, app-list diffs, and
//! (Phase 7) the user-owned launcher layout. These modules are pure data — they
//! depend only on `std`, `serde`, and other library-layer types
//! ([`crate::ui_model`]). They must not depend on `winit`, `wgpu`, Win32, the
//! app shell, or any feature/renderer/worker module.
//!
//! See `ARCHITECTURE.md > Domain Model` for the target shape.
//!
//! ## Discovery vs. user layout
//!
//! [`app_registry`] owns *what apps exist* (rediscoverable records: name, link
//! path, icon, atlas slot). [`launcher_state`] owns *how the user arranged
//! them* (item order, folders, hidden apps). The split means an app being
//! added/removed/re-detected by the OS cannot corrupt the user's arrangement.

pub mod app_diff;
pub mod app_id;
pub mod app_registry;
pub mod folders;
pub mod launcher_item;
pub mod launcher_state;
pub mod settings;
