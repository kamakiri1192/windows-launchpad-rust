//! Edit-mode state types and mutators.

use std::time::Instant;

/// Edit-mode state. The app boundary (`main.rs`) owns the source of truth for
/// the fields that the renderer/scroller read directly (`editing`, `drag_app`,
/// `drag_x`, `drag_y`, `wiggle_phase`); this struct is the feature-side mirror
/// the decision functions operate on. The boundary keeps them in sync.
#[derive(Debug, Clone, Default)]
pub struct EditModeState {
    pub editing: bool,
    pub drag_item: Option<crate::domain::launcher_item::LauncherItem>,
    pub drag_x: f32,
    pub drag_y: f32,
    pub wiggle_phase: f32,
    /// Whether a pending press is currently held. The feature module does not
    /// own `PendingPress` (Phase 5); this is a boolean mirror used so
    /// `enter`/`exit` can request `ClearPendingPress`.
    pub pending_press: bool,
}

impl EditModeState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Snapshot of the pointer at one instant, in physical px.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerSnapshot {
    pub x: f32,
    pub y: f32,
}

impl PointerSnapshot {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Snapshot of a pending press, mirroring the fields of `main.rs::PendingPress`
/// that the long-press decision needs. Built at the call site from the real
/// `PendingPress` so the feature module does not depend on the binary-only type
/// (and so a future Phase 5 move of `PendingPress` to the app shell does not
/// churn this module).
#[derive(Debug, Clone, Copy)]
pub struct PressSnapshot {
    pub start: Instant,
    pub x: f32,
    pub y: f32,
    pub outside_glass: bool,
    pub pointer: PointerSnapshot,
}
