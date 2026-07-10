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

use crate::app_diff::AppDiff;
use crate::app_id::AppId;
use crate::app_registry::AppLaunchInfo;

use crate::settings::SettingsCategory;

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
    Sort(crate::settings::SortOrder),
    FrequentToggle,
    SearchHiddenToggle,
    ResetCache,
    ResetSettings,
    Inside,
    Outside,
}

// ---------------------------------------------------------------------------
// Input routing enums (pure decisions produced by `app::input`).
//
// These describe *what should happen* for a given raw event + shell state.
// They carry no side effects; the handler turns them into method calls on the
// update/command/frame layers. Keeping them pure-data makes the precedence
// rules (settings > edit > search > launcher hide, and
// settings > control > edit/grid) deterministic and unit-testable.
// ---------------------------------------------------------------------------

/// How a pressed key should be routed. Order matters: it mirrors the historical
/// `WindowEvent::KeyboardInput` match arms exactly (settings Esc > edit Esc >
/// search field > launcher hide / debug keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardRoute {
    /// Esc while the settings overlay is open → close settings (no launcher
    /// hide, no passthrough).
    CloseSettings,
    /// Esc while editing → exit edit mode (no launcher hide).
    ExitEditMode,
    /// Esc while the search field wants keyboard → close the field + clear
    /// query (no launcher hide).
    SearchEscClose,
    /// Backspace inside the search field (preedit empty).
    SearchBackspace,
    /// Left arrow inside the search field (preedit empty).
    SearchLeft,
    /// Right arrow inside the search field (preedit empty).
    SearchRight,
    /// A printable character typed into the search field.
    SearchChar(String),
    /// Esc with nothing else open → hide the launcher (stay resident).
    HideLauncher,
    /// `M` debug key → toggle OS window decorations.
    ToggleDecorations,
    /// `R` debug key (only when the search field does not want keyboard) →
    /// reset the icon cache and re-extract.
    ResetIcons,
    /// A Liquid Glass debug key delegated to the renderer.
    LiquidGlassKey(winit::keyboard::KeyCode),
    /// Not handled by the shell (fall through).
    None,
}

/// How a left-button press should be routed. Order mirrors the historical
/// `MouseInput::Pressed` arms: settings overlay first, then the bottom control,
/// then edit mode, then the normal grid press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PressRoute {
    /// Settings overlay open → swallow the press; the release decides close vs
    /// inside-row action. Carries the press-time hit target.
    Settings(super::state::SettingsPressTarget),
    /// Press started on the bottom-control capsule (or edit gear) → mark
    /// `pressed_on_control`; the release re-tests the capsule and dispatches.
    Control,
    /// Editing + press on the grid → hide-app / start-drag / exit (classified
    /// by [`crate::features::edit_mode::edit_press_classify`]).
    EditGrid,
    /// Normal mode grid press → begin a pending press (long-press / click /
    /// scroll-drag resolution deferred).
    GridPress,
    /// Not handled (non-left button, etc.).
    None,
}

/// How a left-button release should be routed. Order mirrors the historical
/// `MouseInput::Released` arms.
#[derive(Debug, Clone)]
pub enum ReleaseRoute {
    /// Settings overlay: outside-press + outside-release → dismiss (no
    /// passthrough). Inside-press + matching inside-release → run the row
    /// action.
    SettingsOutsideDismiss,
    SettingsInside(super::state::SettingsPressTarget),
    /// Control capsule release that stayed on the capsule → control click.
    ControlClick,
    /// Edit-mode drag release → drop + persist.
    EditDrop,
    /// Pending press: stationary release outside the frame → hide + click
    /// passthrough.
    PendingOutsidePassthrough,
    /// Pending press: stationary release over the press-time app id → launch.
    PendingLaunch(AppId),
    /// Scroller drag release (no pending press) → resolve click-or-drag, then
    /// drag end.
    ScrollerRelease,
    /// Nothing to release.
    None,
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
        let launch = AppCommand::LaunchApp(crate::app_registry::AppLaunchInfo {
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

    /// Closing the search field (`press_close`) clears the query, caret, and
    /// preedit before collapsing. That state-machine behavior lives in
    /// `bottom_control::BottomControl`; this test documents that the
    /// `SearchEscClose` keyboard route is the command-boundary representation
    /// of "Esc closes the field (not the launcher)", distinct from
    /// `HideLauncher`.
    #[test]
    fn search_esc_close_is_distinct_from_launcher_hide() {
        let esc_in_field = KeyboardRoute::SearchEscClose;
        let esc_hide = KeyboardRoute::HideLauncher;
        assert_eq!(esc_in_field, KeyboardRoute::SearchEscClose);
        assert_eq!(esc_hide, KeyboardRoute::HideLauncher);
        assert_ne!(esc_in_field, esc_hide);
    }
}
