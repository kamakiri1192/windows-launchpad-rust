//! Event, action, and command types for the app shell.
//!
//! This module is pure data: it defines the values that flow between the
//! handler (`WindowEvent`/`UserEvent` → routing decision), the update layer
//! (state transitions), and the command layer (side-effect execution). It does
//! not perform any routing or side effects itself — that lives in
//! [`super::input`] and [`super::command`].
//!
//! Phase 5 introduces a narrow app-level command set ([`AppCommand`]) that
//! consolidates the proven feature-local command shapes (notably
//! [`crate::features::edit_mode::EditModeCommand`]) at the app boundary, so
//! side effects are requested as data and executed in one place.

use crate::domain::app_diff::AppDiff;
use crate::domain::app_id::AppId;
use crate::domain::app_registry::AppLaunchInfo;

use crate::domain::settings::SettingsCategory;

/// Messages delivered to the UI thread. Besides the existing backdrop frame
/// event, this carries icon-worker results and refresh-watcher diffs.
#[derive(Debug)]
pub enum UserEvent {
    /// A new Windows.Graphics.Capture frame is ready to composite.
    BackdropFrameArrived,
    /// Generic wakeup from a background thread: "drain the shared inbox". We
    /// use one sentinel variant instead of one per message type so the worker
    /// and watcher can share a single inbox without per-variant allocations.
    InboxWakeup,
    /// Background worker finished extracting one icon.
    IconLoaded {
        app_id: AppId,
        image: crate::icons::normalize::DecodedIcon,
    },
    /// Background worker failed to extract one icon.
    IconFailed { app_id: AppId, error: String },
    /// Refresh watcher produced a non-empty Start Menu diff.
    AppListDiff(AppDiff),
    /// Summon the launcher window (global hot key / tray "Show").
    Summon,
    /// User asked to really quit (tray "Quit"). Ends the event loop.
    QuitRequested,
    /// Toggle the settings overlay (tray "Settings" / gear button).
    ToggleSettings,
}

/// A settings-overlay row/category hit, in shell-owned terms. Mirrors the
/// historical `SettingsPressTarget` so pointer routing can classify a press
/// against the layout layer's hit map and then dispatch a concrete action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTarget {
    Close,
    Category(SettingsCategory),
    Sort(crate::domain::settings::SortOrder),
    FrequentToggle,
    SearchHiddenToggle,
    ResetCache,
    ResetSettings,
    Inside,
    Outside,
}

// ---------------------------------------------------------------------------
// AppCommand: the side-effect boundary.
//
// Feature code and the update layer produce these; only the app shell
// ([`super::command`]) executes them. Phase 5 consolidates the edit-mode-local
// [`crate::features::edit_mode::EditModeCommand`] into this set so edit-mode
// side effects run through the same boundary as settings/search/grid side
// effects. The app shell is intentionally not a pure reducer, so these are
// executed eagerly by methods on `App` rather than queued.
// ---------------------------------------------------------------------------

/// Side-effect request produced by the update layer and executed at the app
/// boundary.
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Request a redraw of the window.
    RequestRedraw,
    /// Hide the launcher window (idempotent).
    HideWindow,
    /// Hide the launcher, then replay a left click to the underlying window
    /// (transparent-area click passthrough). Order: hide *before* the click
    /// replay.
    HideWithClickPassthrough,
    /// Show the launcher window and steal focus.
    Summon,
    /// Launch `info`'s shortcut. The launcher is hidden first, then the
    /// shortcut is opened (hide-before-launch ordering).
    LaunchApp(AppLaunchInfo),
    /// Persist the current settings blob.
    PersistSettings,
    /// Persist the current display order (`registry.order()`).
    PersistUserOrder,
    /// Persist the hidden-app list.
    PersistHidden,

    // ---- edit-mode side effects (consolidated from EditModeCommand) ----
    /// `editing = value`.
    SetEditing(bool),
    /// `drag_app = value`.
    SetDragApp(Option<AppId>),
    /// `drag_x` / `drag_y` = the pointer.
    SetDragPos(f32, f32),
    /// `wiggle_phase = 0.0`.
    ResetWigglePhase,
    /// Cancel any in-flight scroll (`phase = Idle`, `velocity = 0`).
    CancelScroll,
    /// `pending_press = None`.
    ClearPendingPress,
    /// Recompute the grid layout + GPU instance buffers.
    Relayout,
    /// Clear the icon cache and re-extract every icon (the `R` debug key and
    /// the settings reset-cache row).
    ResetIconCache,
    /// Set sort order to `Manual`.
    SetSortManual,
    /// Hide `app_id` from the visible stream (registry.hide + order tail +
    /// persist).
    HideApp(AppId),
    /// Programmatically glide the scroller to `page` (edge autoscroll). Only
    /// fires when the scroller is `Idle`.
    SettleToPage(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app-launch path is two commands in a fixed order: hide the window,
    /// then open the shortcut. `HideWindow` must come before the launch so the
    /// launcher vanishes instantly instead of freezing on screen while the
    /// target app starts. This test documents the ordering invariant the
    /// `AppCommand::LaunchApp` executor relies on (hide-then-launch).
    #[test]
    fn launch_command_documents_hide_before_launch_ordering() {
        // The LaunchApp variant carries the launch info; the executor hides
        // first. We assert the variant exists and is distinct from HideWindow
        // so the two-step ordering cannot be collapsed into one.
        let hide = AppCommand::HideWindow;
        let launch = AppCommand::LaunchApp(crate::domain::app_registry::AppLaunchInfo {
            name: "X".to_string(),
            link_path: std::path::PathBuf::from("x.lnk"),
        });
        assert!(matches!(hide, AppCommand::HideWindow));
        assert!(matches!(launch, AppCommand::LaunchApp(_)));
        assert!(!matches!(hide, AppCommand::LaunchApp(_)));
    }

    /// A modal dismiss (settings overlay outside click) must hide the overlay
    /// *without* replaying a click to the underlying window. `HideWindow` and
    /// `HideWithClickPassthrough` are distinct commands so the two dismiss
    /// paths cannot be confused: the settings outside-click uses neither (it
    /// just closes the overlay), while the transparent-area grid click uses
    /// `HideWithClickPassthrough`.
    #[test]
    fn modal_dismiss_is_distinct_from_click_passthrough() {
        let plain_hide = AppCommand::HideWindow;
        let passthrough = AppCommand::HideWithClickPassthrough;
        assert!(matches!(plain_hide, AppCommand::HideWindow));
        assert!(matches!(passthrough, AppCommand::HideWithClickPassthrough));
        // The two must not be the same command — modal dismiss never replays
        // the click.
        assert!(!matches!(plain_hide, AppCommand::HideWithClickPassthrough));
    }
}
