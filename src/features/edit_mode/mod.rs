//! Edit-mode feature: long-press entry, drag state, reorder, hide.
//!
//! This is the Phase 4 feature module described in
//! `docs/DF_REARCHITECTURE_PLAN.md`. It owns edit-mode state transitions,
//! intent classification (long-press entry, edit-press classify, edit-release
//! outcome), the reorder order computation, and the edge-autoscroll zone
//! decision. Side effects (registry mutation, persistence, scroller mutation,
//! redraw) are requested through the narrow [`EditModeCommand`] set and executed
//! by the app boundary (`main.rs`).
//!
//! Behavior preservation: every decision here is the exact logic the historical
//! `main.rs` helpers (`maybe_long_press_into_edit`, the `MouseInput` edit-mode
//! branches, `live_reorder`, `commit_reorder`, `hide_app`) performed inline. The
//! feature module decides intent and the new order; the app boundary runs the
//! `registry.set_order` / `registry.hide` / `scroller.settle_to_page` /
//! `request_redraw` side effects.
//!
//! A global `AppAction` / `AppCommand` is intentionally **not** introduced here
//! — that is Phase 5. This module exposes only edit-mode-specific types.
//!
//! Phase 4 notes:
//! - The feature module lives in the binary tree (it depends on `app_id::AppId`,
//!   which has not yet moved to `domain/`). The migration to `domain/` is
//!   Phase 7/8.
//! - GPU-facing `TileAnim` / `TileInstance` / `IconInstance` and the
//!   renderer badge source stay in `main.rs` / `grid.rs` / `renderer.rs`. This
//!   module does not touch GPU data.

mod state;

pub use state::{EditModeState, PointerSnapshot, PressSnapshot};

use crate::app_id::AppId;
use crate::layout::grid::GridHit;
use std::time::{Duration, Instant};

/// How long a press must be held (without dragging past slop) to enter edit
/// mode. Mirrors `main.rs::LONG_PRESS_THRESHOLD`. Re-declared here so the
/// pure long-press decision is self-contained and testable from the library-
/// style binary feature module.
pub const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(500);

/// Press slop (physical px). A press that moves more than this is not a click
/// and not a long-press (it becomes a scroll drag). Mirrors
/// `main.rs::CLICK_SLOP_PHYS`.
pub const CLICK_SLOP_PHYS: f32 = 8.0;

/// True if `(release - start)` is within the click slop radius. Shared by
/// long-press entry (a long-press requires the pointer to stay within slop),
/// click classification, and the outside-glass click check.
pub fn within_click_slop(dx: f32, dy: f32) -> bool {
    dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS
}

/// Decide whether a pending press should promote into edit mode right now.
///
/// Mirrors the historical `maybe_long_press_into_edit`:
/// - no pending press → no;
/// - the press started outside the page glass (`outside_glass`) → no (that is
///   the click-passthrough path, never edit mode);
/// - the pointer has since moved past `CLICK_SLOP_PHYS` → no (it is becoming a
///   scroll drag);
/// - `now - start < LONG_PRESS_THRESHOLD` → not yet;
/// - otherwise → yes.
///
/// `PressSnapshot` is built from `main.rs::PendingPress` at the call site; the
/// feature module does not own `PendingPress` directly (that type also drives
/// launch/passthrough/scroll-drag and is migrated to the app shell in Phase 5).
pub fn should_enter_from_long_press(press: &PressSnapshot, now: Instant) -> bool {
    if press.outside_glass {
        return false;
    }
    if !within_click_slop(press.pointer.x - press.x, press.pointer.y - press.y) {
        return false;
    }
    now.duration_since(press.start) >= LONG_PRESS_THRESHOLD
}

/// What an edit-mode press on the grid should do. Classified from the grid hit
/// and the badge hit at press time. Mirrors the `MouseInput::Pressed` edit-mode
/// branch: badge hit takes precedence over a drag (hide), a real app becomes a
/// drag, and empty space inside the frame exits edit mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditPressIntent {
    /// Pointer is over a visible app's ✕ badge → hide that app.
    HideApp { visible_index: usize },
    /// Pointer is over a visible app (not its badge) → start dragging it.
    DragApp { visible_index: usize },
    /// Pointer is over empty space inside the frame → exit edit mode.
    EmptyExit,
    /// Pointer is not over anything actionable (outside the frame, or a hit the
    /// grid classifier did not produce). The app boundary decides the default
    /// (currently: fall through / no-op).
    Noop,
}

/// Classify an edit-mode press on the grid into an [`EditPressIntent`].
///
/// `app_hit` is the grid classifier result ([`GridHit::App`] when over a
/// visible app; `EmptyInFrame` / `OutsideFrame` otherwise). `badge_hit` is
/// whether the pointer is over that app's ✕ badge hit circle (the app boundary
/// computes it via [`crate::layout::edit_mode::badge_hit`]).
pub fn edit_press_classify(app_hit: GridHit, badge_hit: bool) -> EditPressIntent {
    match app_hit {
        GridHit::App(idx) if badge_hit => EditPressIntent::HideApp { visible_index: idx },
        GridHit::App(idx) => EditPressIntent::DragApp { visible_index: idx },
        GridHit::EmptyInFrame => EditPressIntent::EmptyExit,
        GridHit::OutsideFrame => EditPressIntent::Noop,
    }
}

/// Narrow edit-mode side-effect request. The feature module returns a list of
/// these; the app boundary (`main.rs`) executes them. Phase 5 will consolidate
/// this into the global `AppCommand`, but Phase 4 keeps it edit-mode-local.
//
// `PartialEq` (not `Eq`) because `SetDragPos` carries `f32` coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum EditModeCommand {
    /// `editing = value`. The first transition logs "edit-mode: entered/exited".
    SetEditing(bool),
    /// `drag_app = value`. `None` clears an in-flight drag.
    SetDragApp(Option<AppId>),
    /// `drag_x` / `drag_y` = the pointer (the lifted tile follows the pointer).
    SetDragPos(f32, f32),
    /// `wiggle_phase = 0.0` (reset on entry so the wiggle starts fresh).
    ResetWigglePhase,
    /// Cancel any in-flight scroll: scroller `phase = Idle`, `velocity = 0`.
    /// Only meaningful when entering edit mode.
    CancelScroll,
    /// `pending_press = None` (the long-press press is consumed on entry; an
    /// empty-click exit also drops any pending press).
    ClearPendingPress,
    /// Recompute the grid layout + GPU instance buffers (page count, springs).
    Relayout,
    /// Request a redraw.
    RequestRedraw,
    /// Persist the current display order (`registry.order()`) so reorder
    /// survives a restart.
    PersistUserOrder,
    /// Persist the hidden-app list so hide survives a restart.
    PersistHidden,
    /// Persist settings (used after a reorder commits `SortOrder::Manual`).
    PersistSettings,
    /// Set sort order to `Manual`. Emitted alongside `PersistSettings` when a
    /// reorder commits so subsequent name-sorts don't clobber the user layout.
    SetSortManual,
    /// Hide `app_id` from the visible stream. The app boundary runs
    /// `registry.hide`, moves the id to the tail of the order, relayouts, and
    /// persists.
    HideApp(AppId),
    /// Programmatically glide the scroller to `page` (edge autoscroll). The
    /// app boundary only fires `settle_to_page` when the scroller is `Idle`.
    SettleToPage(usize),
}

/// Enter edit mode, optionally lifting `app_index` straight into a drag (the
/// long-press path). Returns the commands the app boundary should execute.
///
/// Mirrors the historical `enter_edit_mode`:
/// - `editing = true` (idempotent; the boundary logs only on first transition);
/// - `pending_press = None`;
/// - `wiggle_phase = 0.0`;
/// - cancel any in-flight scroll;
/// - if `app_index` resolves to a visible app, lift it into a drag at the
///   current pointer;
/// - relayout + redraw.
///
/// `visible_ids` is the current visible stream; `app_index` is the visible
/// index the long-press started over (may be `None` for an empty long-press,
/// in which case no app is lifted).
pub fn enter(
    state: &mut EditModeState,
    app_index: Option<usize>,
    visible_ids: &[AppId],
    pointer: PointerSnapshot,
) -> Vec<EditModeCommand> {
    state.editing = true;
    state.pending_press = false;
    state.wiggle_phase = 0.0;
    let mut cmds = vec![
        EditModeCommand::SetEditing(true),
        EditModeCommand::ClearPendingPress,
        EditModeCommand::ResetWigglePhase,
        EditModeCommand::CancelScroll,
    ];
    if let Some(idx) = app_index {
        if let Some(id) = visible_ids.get(idx).cloned() {
            state.drag_app = Some(id.clone());
            state.drag_x = pointer.x;
            state.drag_y = pointer.y;
            cmds.push(EditModeCommand::SetDragApp(Some(id)));
            cmds.push(EditModeCommand::SetDragPos(pointer.x, pointer.y));
        }
    }
    cmds.push(EditModeCommand::Relayout);
    cmds.push(EditModeCommand::RequestRedraw);
    cmds
}

/// Exit edit mode, committing any in-flight drag first. Returns the commands
/// the app boundary should execute.
///
/// Mirrors the historical `exit_edit_mode`:
/// - if a drag is in flight, finalize it (`commit_reorder` equivalent:
///   drop-at-current-cell + persist);
/// - `editing = false`;
/// - `drag_app = None`;
/// - `pending_press = None`;
/// - relayout + redraw.
///
/// `commit_commands` is the list of commands produced by [`commit_drag`] (the
/// app boundary runs them as part of the exit). This keeps the commit logic in
/// one place.
pub fn exit(
    state: &mut EditModeState,
    commit_commands: Vec<EditModeCommand>,
) -> Vec<EditModeCommand> {
    let mut cmds = Vec::new();
    if state.drag_app.is_some() {
        cmds.extend(commit_commands);
    }
    state.editing = false;
    state.drag_app = None;
    state.pending_press = false;
    cmds.push(EditModeCommand::SetEditing(false));
    cmds.push(EditModeCommand::SetDragApp(None));
    cmds.push(EditModeCommand::ClearPendingPress);
    cmds.push(EditModeCommand::Relayout);
    cmds.push(EditModeCommand::RequestRedraw);
    cmds
}

/// Start dragging the visible app at `visible_index`. Mirrors the edit-mode
/// press branch's drag-start path.
pub fn start_drag(
    state: &mut EditModeState,
    visible_ids: &[AppId],
    visible_index: usize,
    pointer: PointerSnapshot,
) -> Vec<EditModeCommand> {
    let Some(id) = visible_ids.get(visible_index).cloned() else {
        return Vec::new();
    };
    state.drag_app = Some(id.clone());
    state.drag_x = pointer.x;
    state.drag_y = pointer.y;
    vec![
        EditModeCommand::SetDragApp(Some(id)),
        EditModeCommand::SetDragPos(pointer.x, pointer.y),
        EditModeCommand::Relayout,
        EditModeCommand::RequestRedraw,
    ]
}

/// Update the dragged tile's follow position during a move and request a redraw.
/// The app boundary runs `live_reorder` / edge-autoscroll around this; this
/// helper only updates the stored drag position. Mirrors `handle_edit_drag_move`.
pub fn drag_move(state: &mut EditModeState, pointer: PointerSnapshot) -> Vec<EditModeCommand> {
    state.drag_x = pointer.x;
    state.drag_y = pointer.y;
    vec![
        EditModeCommand::SetDragPos(pointer.x, pointer.y),
        EditModeCommand::RequestRedraw,
    ]
}

/// Commit an in-flight drag: drop at the current cell and persist. Mirrors
/// `commit_reorder` (the persist side) — the actual order mutation is produced
/// by [`apply_reorder`] and run by the app boundary.
///
/// Returns the commands to execute: set sort order to `Manual`, persist
/// settings, persist user order.
pub fn commit_drag(state: &EditModeState) -> Vec<EditModeCommand> {
    // If there is no drag, commit is a no-op (the boundary may still need to
    // clear drag_app via `exit`).
    if state.drag_app.is_none() {
        return Vec::new();
    }
    vec![
        EditModeCommand::SetSortManual,
        EditModeCommand::PersistSettings,
        EditModeCommand::PersistUserOrder,
    ]
}

/// Compute the new display order after moving `drag_id` to `insert_idx` in the
/// visible stream, preserving hidden apps after the visible stream. Mirrors the
/// historical `reorder_by_index` order computation (the boundary calls
/// `registry.set_order` with the result).
///
/// Returns `None` if `drag_id` is not present in `visible_ids` and not in
/// `hidden_ids` (the historical `reorder_by_index` early-returned in that case).
/// Otherwise returns the new order and whether a reorder actually happened (the
/// order differs from `visible + hidden`).
pub fn apply_reorder(
    visible_ids: &[AppId],
    hidden_ids: &[AppId],
    drag_id: &AppId,
    insert_idx: usize,
) -> Option<Vec<AppId>> {
    // Build the current order: visible stream, then hidden apps (preserved but
    // never repositioned visibly). This matches the historical chain().
    let mut order: Vec<AppId> = visible_ids
        .iter()
        .chain(hidden_ids.iter())
        .cloned()
        .collect();
    let drag_pos = order.iter().position(|i| i == drag_id)?;
    let id = order.remove(drag_pos);
    order.insert(insert_idx.min(order.len()), id);
    Some(order)
}

/// Compute the order change for `hide_app`: the hidden id is moved to the tail
/// of the order so it does not linger invisibly mid-grid. Mirrors the historical
/// `hide_app` order computation.
pub fn hidden_order_after_hide(order: &[AppId], id: &AppId) -> Vec<AppId> {
    let mut new_order: Vec<AppId> = order.iter().filter(|x| *x != id).cloned().collect();
    new_order.push(id.clone());
    new_order
}

#[cfg(test)]
mod tests;
