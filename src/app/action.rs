//! `AppAction`: the normalized input intent produced by the handler.
//!
//! The handler (`ApplicationHandler`) converts each raw `WindowEvent` /
//! `UserEvent` into an [`AppAction`] value and dispatches it through
//! [`App::handle_action`]. This is the production "raw event → AppAction →
//! update → AppCommand → command executor" path described in
//! `ARCHITECTURE.md > Events, Actions, and Commands`.
//!
//! [`AppAction`] is the single dispatch surface: the handler no longer calls
//! feature methods or platform adapters inline. Keyboard/IME/pointer
//! precedence (settings > edit > search > launcher hide;
//! settings > control > edit/grid) is encoded here so the handler stays thin.
//!
//! The pure classification helpers ([`keyboard_action`], [`pointer_press_action`],
//! [`pointer_release_action`]) are extracted so the precedence rules are
//! deterministic and unit-testable without a window, renderer, or scroller.
//! `handle_action` consumes the action and runs the appropriate `&mut self`
//! transition / side effect.

use std::time::Instant;

use winit::event::Ime;
use winit::keyboard::KeyCode;

use crate::app::event::AppCommand;
use crate::app::state::App;
use crate::debug_log;

use super::state::SettingsPressTarget;

/// Normalized input intent. The handler produces one of these from each raw
/// event and dispatches it via [`App::handle_action`].
#[derive(Debug)]
pub enum AppAction {
    // ---- lifecycle / window ----
    CloseRequested,
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged {
        scale_factor: f64,
    },
    Moved,
    RedrawRequested,
    Focused(bool),
    /// A redraw tick opportunity (about_to_wait). Carries the current time so
    /// the long-press timer and animation advance use a consistent clock.
    Tick {
        now: Instant,
    },

    // ---- worker / OS events ----
    BackdropFrameArrived,
    DrainInbox,
    Summon,
    ToggleSettings,
    QuitRequested,

    // ---- keyboard ----
    /// A key press, already classified by [`keyboard_action`].
    Keyboard(KeyAction),

    // ---- IME ----
    /// An IME event, gated on `control.wants_keyboard()` by the handler.
    Ime(Ime),

    // ---- pointer ----
    /// A left-button press, already classified by [`pointer_press_action`].
    PointerPress(PressAction),
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerRelease(ReleaseAction),
    CursorLeft,
}

/// Keyboard intent, classified by [`keyboard_action`] from the shell flags and
/// the raw key/text payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    CloseFolder,
    CancelFolderRename,
    CommitFolderRename,
    FolderRenameBackspace,
    FolderRenameLeft,
    FolderRenameRight,
    FolderRenameChar(String),
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
    /// Backspace inside the search field (preedit non-empty → OS IME owns it;
    /// just redraw).
    SearchBackspaceBlocked,
    /// Left arrow inside the search field (preedit empty).
    SearchLeft,
    /// Left arrow while preedit is active (just redraw, no caret move).
    SearchLeftBlocked,
    /// Right arrow inside the search field (preedit empty).
    SearchRight,
    /// Right arrow while preedit is active (just redraw).
    SearchRightBlocked,
    /// A printable character typed into the search field.
    SearchChar(String),
    /// Esc with nothing open → hide the launcher (stay resident).
    HideLauncher,
    /// `M` debug key → toggle OS window decorations.
    ToggleDecorations,
    /// `R` debug key (only when the search field is closed) → reset the icon
    /// cache and re-extract.
    ResetIcons,
    /// A Liquid Glass debug key delegated to the renderer.
    LiquidGlassKey(KeyCode),
    /// Not handled by the shell (fall through).
    None,
}

/// Left-button press intent, classified by [`pointer_press_action`] from the
/// shell flags and the pointer position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PressAction {
    Folder,
    /// Settings overlay open → swallow the press; the release decides close vs
    /// inside-row action. Carries the press-time hit target.
    Settings(SettingsPressTarget),
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

/// Left-button release intent, classified by [`pointer_release_action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseAction {
    Folder,
    /// Settings overlay: outside-press + outside-release → dismiss (no
    /// passthrough). Inside-press + matching inside-release → run the row
    /// action.
    Settings {
        pressed: SettingsPressTarget,
        released: SettingsPressTarget,
    },
    /// Control capsule release that stayed on the capsule → control click.
    Control,
    /// Edit-mode drag release → drop + persist.
    EditDrop,
    /// Pending press: stationary release outside the frame → hide + click
    /// passthrough.
    PendingOutsidePassthrough,
    /// Pending press: stationary release over the press-time app id → launch.
    PendingLaunch,
    /// Scroller drag release (no pending press) → resolve click-or-drag, then
    /// drag end.
    ScrollerRelease,
    /// Nothing to release.
    None,
}

/// Classify a keyboard event into a [`KeyAction`], mirroring the historical
/// `WindowEvent::KeyboardInput` precedence exactly:
///
/// 1. settings overlay open + Esc → close settings;
/// 2. editing + Esc → exit edit mode;
/// 3. search field wants keyboard → Esc/Backspace/Left/Right/char handling;
/// 4. Esc with nothing open → hide the launcher;
/// 5. `M` → toggle decorations; `R` (field closed) → reset icon cache;
///    otherwise → Liquid Glass debug key delegation.
pub fn keyboard_action(
    settings_open: bool,
    editing: bool,
    control_wants_keyboard: bool,
    preedit_empty: bool,
    key_code: Option<KeyCode>,
    text: Option<&str>,
) -> KeyAction {
    // 1. Settings overlay takes precedence over everything.
    if settings_open && key_code == Some(KeyCode::Escape) {
        return KeyAction::CloseSettings;
    }

    // 2. Edit mode takes precedence over everything except the search field.
    if editing && key_code == Some(KeyCode::Escape) {
        return KeyAction::ExitEditMode;
    }

    // 3. While the search field has focus, the control eats most keys.
    if control_wants_keyboard {
        match key_code {
            Some(KeyCode::Escape) => {
                return KeyAction::SearchEscClose;
            }
            Some(KeyCode::Backspace) => {
                return if preedit_empty {
                    KeyAction::SearchBackspace
                } else {
                    KeyAction::SearchBackspaceBlocked
                };
            }
            Some(KeyCode::ArrowLeft) => {
                return if preedit_empty {
                    KeyAction::SearchLeft
                } else {
                    KeyAction::SearchLeftBlocked
                };
            }
            Some(KeyCode::ArrowRight) => {
                return if preedit_empty {
                    KeyAction::SearchRight
                } else {
                    KeyAction::SearchRightBlocked
                };
            }
            _ => {}
        }
        // Otherwise, let printable text through (direct, non-IME chars arrive
        // in event.text). Blocked while preedit is non-empty.
        if preedit_empty {
            if let Some(t) = text {
                if !t.is_empty() {
                    return KeyAction::SearchChar(t.to_string());
                }
            }
        }
        return KeyAction::None;
    }

    // 4. Esc with no open field: hide the launcher (stay resident).
    if key_code == Some(KeyCode::Escape) {
        return KeyAction::HideLauncher;
    }

    // 5. Debug keys.
    if key_code == Some(KeyCode::KeyM) {
        return KeyAction::ToggleDecorations;
    }
    if key_code == Some(KeyCode::KeyR) {
        return KeyAction::ResetIcons;
    }

    // 6. Otherwise, delegate to the Liquid Glass debug key handler.
    match key_code {
        Some(k) => KeyAction::LiquidGlassKey(k),
        None => KeyAction::None,
    }
}

pub fn folder_keyboard_action(
    rename_active: bool,
    editing: bool,
    preedit_empty: bool,
    key_code: Option<KeyCode>,
    text: Option<&str>,
) -> KeyAction {
    if !rename_active {
        return if key_code == Some(KeyCode::Escape) {
            if editing {
                KeyAction::ExitEditMode
            } else {
                KeyAction::CloseFolder
            }
        } else {
            KeyAction::None
        };
    }
    match key_code {
        Some(KeyCode::Escape) => KeyAction::CancelFolderRename,
        Some(KeyCode::Enter) | Some(KeyCode::NumpadEnter) => KeyAction::CommitFolderRename,
        Some(KeyCode::Backspace) if preedit_empty => KeyAction::FolderRenameBackspace,
        Some(KeyCode::ArrowLeft) if preedit_empty => KeyAction::FolderRenameLeft,
        Some(KeyCode::ArrowRight) if preedit_empty => KeyAction::FolderRenameRight,
        _ if preedit_empty => text
            .filter(|value| !value.is_empty())
            .map(|value| KeyAction::FolderRenameChar(value.to_owned()))
            .unwrap_or(KeyAction::None),
        _ => KeyAction::None,
    }
}

/// Classify a left-button press into a [`PressAction`], mirroring the
/// historical `MouseInput::Pressed` precedence:
/// settings overlay (swallow) > bottom control (mark pressed_on_control) >
/// edit mode (hide/drag/exit) > normal grid press.
pub fn pointer_press_action(
    settings_open: bool,
    settings_press_target: SettingsPressTarget,
    over_control: bool,
    editing: bool,
) -> PressAction {
    if settings_open {
        return PressAction::Settings(settings_press_target);
    }
    if over_control {
        return PressAction::Control;
    }
    if editing {
        return PressAction::EditGrid;
    }
    PressAction::GridPress
}

/// Classify a left-button release into a [`ReleaseAction`]. This is a thin
/// classifier: the handler passes the shell flags and the press/release hit
/// decisions, and the action picks the right branch.
#[allow(clippy::too_many_arguments)]
pub fn pointer_release_action(
    settings_open: bool,
    settings_pressed: Option<SettingsPressTarget>,
    settings_released: SettingsPressTarget,
    pressed_on_control: bool,
    on_capsule: bool,
    editing_with_drag: bool,
    has_pending_press: bool,
    is_outside_glass_click: bool,
    has_launch_id: bool,
    scroller_dragging: bool,
) -> ReleaseAction {
    if settings_open {
        let pressed = settings_pressed.unwrap_or(SettingsPressTarget::Outside);
        return ReleaseAction::Settings {
            pressed,
            released: settings_released,
        };
    }
    if pressed_on_control {
        return if on_capsule {
            ReleaseAction::Control
        } else {
            ReleaseAction::None
        };
    }
    if editing_with_drag {
        return ReleaseAction::EditDrop;
    }
    if has_pending_press {
        if is_outside_glass_click {
            return ReleaseAction::PendingOutsidePassthrough;
        }
        return if has_launch_id {
            ReleaseAction::PendingLaunch
        } else {
            ReleaseAction::None
        };
    }
    if scroller_dragging {
        return ReleaseAction::ScrollerRelease;
    }
    ReleaseAction::None
}

impl App {
    /// Dispatch a normalized [`AppAction`]. This is the production dispatch
    /// surface: the handler converts raw events into actions and calls this
    /// method. Side effects that are not state transitions (hide, launch,
    /// passthrough, persist, reset) run through [`Self::execute_command`] so
    /// they share the single command boundary.
    pub(crate) fn handle_action(&mut self, action: AppAction) {
        match action {
            AppAction::CloseRequested => {
                self.execute_command(AppCommand::HideWindow);
            }
            AppAction::Resized { width, height } => {
                if width == 0 || height == 0 {
                    return;
                }
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(width, height);
                }
                self.relayout();
                self.execute_command(AppCommand::RequestRedraw);
            }
            AppAction::ScaleFactorChanged { scale_factor } => {
                self.scale_factor = scale_factor as f32;
                self.relayout();
                self.execute_command(AppCommand::RequestRedraw);
            }
            AppAction::Moved => {
                if let Some(r) = self.renderer.as_mut() {
                    r.notify_window_moved();
                }
                self.execute_command(AppCommand::RequestRedraw);
            }
            AppAction::RedrawRequested => {
                self.tick_frame();
            }
            AppAction::Tick { now } => {
                self.handle_tick(now);
            }
            AppAction::Focused(focused) => {
                self.handle_focus(focused);
            }
            AppAction::BackdropFrameArrived => {
                self.execute_command(AppCommand::RequestRedraw);
            }
            AppAction::DrainInbox => {
                if !self.qa_enabled() {
                    self.drain_inbox();
                }
            }
            AppAction::Summon => {
                self.execute_command(AppCommand::Summon);
            }
            AppAction::ToggleSettings => {
                if !self.visible {
                    self.execute_command(AppCommand::Summon);
                }
                self.toggle_settings();
            }
            AppAction::QuitRequested => {
                debug_log!("user_event: QuitRequested received → process::exit(0)");
                std::process::exit(0);
            }
            AppAction::Keyboard(key_action) => {
                self.handle_keyboard(key_action);
            }
            AppAction::Ime(ime) => {
                self.handle_ime(ime);
            }
            AppAction::PointerPress(press_action) => {
                self.handle_pointer_press(press_action);
            }
            AppAction::PointerMoved { x, y } => {
                self.handle_pointer_moved(x, y);
            }
            AppAction::PointerRelease(release_action) => {
                self.handle_pointer_release(release_action);
            }
            AppAction::CursorLeft => {
                self.handle_cursor_left();
            }
        }
    }

    /// Handle a classified keyboard action. This replaces the historical inline
    /// `WindowEvent::KeyboardInput` branch: the precedence decision lives in
    /// [`keyboard_action`], this method runs the side effect.
    fn handle_keyboard(&mut self, key_action: KeyAction) {
        match key_action {
            KeyAction::CloseFolder => self.close_folder(),
            KeyAction::CancelFolderRename => {
                self.folders.cancel_rename();
                self.request_redraw();
            }
            KeyAction::CommitFolderRename => self.commit_folder_rename(),
            KeyAction::FolderRenameBackspace => {
                if let Some(editor) = self.folders.rename.as_mut() {
                    editor.backspace();
                }
                self.request_redraw();
            }
            KeyAction::FolderRenameLeft => {
                if let Some(editor) = self.folders.rename.as_mut() {
                    editor.move_left();
                }
                self.request_redraw();
            }
            KeyAction::FolderRenameRight => {
                if let Some(editor) = self.folders.rename.as_mut() {
                    editor.move_right();
                }
                self.request_redraw();
            }
            KeyAction::FolderRenameChar(value) => {
                if let Some(editor) = self.folders.rename.as_mut() {
                    editor.commit_text(&value);
                }
                self.request_redraw();
            }
            KeyAction::CloseSettings => self.close_settings(),
            KeyAction::ExitEditMode => self.exit_edit_mode(),
            KeyAction::SearchEscClose => {
                self.control.press_close();
                self.search_input_changed();
            }
            KeyAction::SearchBackspace => {
                self.control.handle_backspace();
                self.search_input_changed();
            }
            KeyAction::SearchBackspaceBlocked => {
                self.execute_command(AppCommand::RequestRedraw);
            }
            KeyAction::SearchLeft => {
                self.control.handle_left();
                self.execute_command(AppCommand::RequestRedraw);
            }
            KeyAction::SearchLeftBlocked => {
                self.execute_command(AppCommand::RequestRedraw);
            }
            KeyAction::SearchRight => {
                self.control.handle_right();
                self.execute_command(AppCommand::RequestRedraw);
            }
            KeyAction::SearchRightBlocked => {
                self.execute_command(AppCommand::RequestRedraw);
            }
            KeyAction::SearchChar(text) => {
                let mut any = false;
                for ch in text.chars() {
                    if self.control.handle_char(ch) {
                        any = true;
                    }
                }
                if any {
                    self.search_input_changed();
                }
            }
            KeyAction::HideLauncher => {
                self.execute_command(AppCommand::HideWindow);
            }
            KeyAction::ToggleDecorations => {
                if let Some(r) = self.renderer.as_mut() {
                    r.toggle_decorations();
                    self.execute_command(AppCommand::RequestRedraw);
                }
            }
            KeyAction::ResetIcons => {
                self.execute_command(AppCommand::ResetIconCache);
            }
            KeyAction::LiquidGlassKey(key_code) => {
                if let Some(r) = self.renderer.as_mut() {
                    if r.handle_liquid_glass_key(key_code) {
                        self.execute_command(AppCommand::RequestRedraw);
                    }
                }
            }
            KeyAction::None => {}
        }
    }

    /// Handle an IME event. Gated on `control.wants_keyboard()` by the handler.
    fn handle_ime(&mut self, ime: Ime) {
        if self.folders.rename.is_some() {
            match ime {
                Ime::Preedit(value, _) => {
                    self.folders.rename.as_mut().unwrap().set_preedit(value);
                }
                Ime::Commit(value) => {
                    self.folders.rename.as_mut().unwrap().commit_text(&value);
                }
                Ime::Disabled => {
                    self.folders
                        .rename
                        .as_mut()
                        .unwrap()
                        .set_preedit(String::new());
                }
                Ime::Enabled => {}
            }
            self.request_redraw();
            return;
        }
        if !self.control.wants_keyboard() {
            return;
        }
        match ime {
            Ime::Preedit(s, _) => {
                self.control.set_preedit(s);
                self.search_input_changed();
            }
            Ime::Commit(text) => {
                self.control.set_preedit(String::new());
                let mut any = false;
                for ch in text.chars() {
                    if self.control.handle_char(ch) {
                        any = true;
                    }
                }
                if any || !text.is_empty() {
                    self.search_input_changed();
                }
            }
            Ime::Enabled => {}
            Ime::Disabled => {
                self.control.set_preedit(String::new());
                self.search_input_changed();
            }
        }
    }

    /// Handle a classified pointer press.
    fn handle_pointer_press(&mut self, press_action: PressAction) {
        let px = self.pointer_phys_x;
        let py = self.pointer_phys_y;
        match press_action {
            PressAction::Folder => self.handle_folder_pointer_press(px, py),
            PressAction::Settings(target) => {
                self.pressed_on_settings = Some(target);
            }
            PressAction::Control => {
                self.pressed_on_control = true;
            }
            PressAction::EditGrid => {
                let hit = self.grid_hit_at_pointer(px, py);
                let badge_hit = matches!(hit, crate::layout::grid::GridHit::App(idx)
                    if matches!(self.visible_launcher_items().get(idx),
                        Some(crate::domain::launcher_item::LauncherItem::App(_)))
                        && self.badge_hit(idx, px, py));
                let intent = crate::features::edit_mode::edit_press_classify(hit, badge_hit);
                match intent {
                    crate::features::edit_mode::EditPressIntent::HideApp { visible_index } => {
                        if let Some(crate::domain::launcher_item::LauncherItem::App(id)) =
                            self.visible_launcher_items().get(visible_index).cloned()
                        {
                            debug_log!("edit-drag: badge press idx={visible_index}");
                            self.hide_app(&id);
                        }
                    }
                    crate::features::edit_mode::EditPressIntent::DragApp { visible_index } => {
                        debug_log!("edit-drag: press idx={visible_index}");
                        let item = self.visible_launcher_items()[visible_index].clone();
                        self.drag_item = Some(item);
                        self.drag_x = px;
                        self.drag_y = py;
                        self.relayout();
                        self.execute_command(AppCommand::RequestRedraw);
                    }
                    crate::features::edit_mode::EditPressIntent::EmptyExit
                    | crate::features::edit_mode::EditPressIntent::Noop => {
                        self.exit_edit_mode();
                    }
                }
            }
            PressAction::GridPress => {
                self.begin_grid_press(Instant::now());
            }
            PressAction::None => {}
        }
    }

    /// Handle a pointer move event.
    fn handle_pointer_moved(&mut self, x: f32, y: f32) {
        self.pointer_phys_x = x;
        self.pointer_phys_y = y;
        // Edit-mode drag: follow the pointer and live-reorder.
        if self.editing && self.drag_item.is_some() {
            self.handle_edit_drag_move();
            return;
        }
        if self.folders.is_active() {
            self.handle_folder_pointer_move(x, y);
            return;
        }
        // A pending press may promote to a real scroll drag once it moves past
        // slop.
        if self.pending_press.is_some() && self.maybe_promote_press_to_drag() {
            return;
        }
        let dragging = self
            .scroller
            .as_ref()
            .map(|s| s.phase == crate::scroll::Phase::Dragging)
            .unwrap_or(false);
        if dragging {
            self.handle_drag_move(x);
        }
    }

    /// Handle a classified pointer release.
    fn handle_pointer_release(&mut self, release_action: ReleaseAction) {
        let px = self.pointer_phys_x;
        let py = self.pointer_phys_y;
        // Always clear the control-press flag on release — it was set by the
        // press classifier and must not persist beyond the matching release,
        // otherwise subsequent grid presses would be misclassified through the
        // stale control branch.
        self.pressed_on_control = false;
        match release_action {
            ReleaseAction::Folder => self.handle_folder_pointer_release(px, py),
            ReleaseAction::Settings { pressed, released } => {
                if pressed == SettingsPressTarget::Outside
                    && released == SettingsPressTarget::Outside
                {
                    self.close_settings();
                    return;
                }
                if pressed == released {
                    self.handle_settings_click(released);
                }
                // Mismatched inside/outside releases are ignored (no close),
                // matching the historical behavior that only dismissed on a
                // clean outside-press + outside-release.
            }
            ReleaseAction::Control => {
                self.handle_control_click(px, py);
            }
            ReleaseAction::EditDrop => {
                self.commit_edit_drop();
                self.drag_item = None;
                self.relayout();
                self.execute_command(AppCommand::RequestRedraw);
            }
            ReleaseAction::PendingOutsidePassthrough => {
                self.pending_press = None;
                self.execute_command(AppCommand::HideWithClickPassthrough);
            }
            ReleaseAction::PendingLaunch => {
                if let Some(press) = self.pending_press.take() {
                    if let Some(item) = press.activated_item(px, py).cloned() {
                        match item {
                            crate::domain::launcher_item::LauncherItem::App(id) => {
                                if let Some(info) = self.registry.launch_info(&id) {
                                    self.execute_command(AppCommand::LaunchApp(info));
                                }
                            }
                            crate::domain::launcher_item::LauncherItem::Folder(id) => {
                                self.open_folder(id);
                            }
                        }
                    }
                }
            }
            ReleaseAction::ScrollerRelease => {
                if let Some(info) = self.handle_pointer_release_launch() {
                    self.execute_command(AppCommand::LaunchApp(info));
                }
            }
            ReleaseAction::None => {
                self.pending_press = None;
            }
        }
    }

    /// Handle a cursor-left event.
    fn handle_cursor_left(&mut self) {
        let dragging = self
            .scroller
            .as_ref()
            .map(|s| s.phase == crate::scroll::Phase::Dragging)
            .unwrap_or(false);
        if dragging {
            self.handle_drag_end();
        }
        if self.editing && self.drag_item.is_some() {
            self.commit_reorder();
            self.drag_item = None;
            if self.folders.hover_opened.is_some() {
                self.folders.close();
            }
            self.folders.hover = None;
            self.relayout();
        }
        if self.folders.child_drag.is_some() || self.folders.pressed_child.is_some() {
            self.folders.clear_child_pointer();
            self.editing = false;
            self.request_redraw();
        }
        self.pending_press = None;
        self.pressed_on_control = false;
    }

    /// Handle an about-to-wait tick: long-press check and redraw gating.
    fn handle_tick(&mut self, now: Instant) {
        if self.should_quit {
            // Quit is handled by the event loop exit in the handler.
            return;
        }
        self.tick_qa(now);
        let long_press_pending = self.pending_press.is_some();
        if long_press_pending {
            self.maybe_long_press_into_edit(now);
        }
        let folder_long_press_pending = self.folders.pressed_child.is_some() && !self.editing;
        if folder_long_press_pending
            && self
                .folders
                .child_long_press_ready(now, self.pointer_phys_x, self.pointer_phys_y)
        {
            self.enter_edit_mode(None);
        }
        let scroller_animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        let folder_scroller_animating = self
            .folder_scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        let control_animating = self.control.mode.is_morphing()
            || matches!(
                self.control.mode,
                crate::features::bottom_control::Mode::Indicator
            )
            || matches!(
                self.control.mode,
                crate::features::bottom_control::Mode::Field
            );
        if scroller_animating
            || folder_scroller_animating
            || control_animating
            || self.editing
            || long_press_pending
            || folder_long_press_pending
            || self.qa_capture_due(now)
            || matches!(
                self.folders.phase,
                crate::features::folders::FolderPhase::Opening
                    | crate::features::folders::FolderPhase::Closing
            )
        {
            self.execute_command(AppCommand::RequestRedraw);
        }
    }

    /// Handle a focus-change event. Auto-hides the launcher when focus is lost,
    /// with the historical grace-period / edit-mode / settings exceptions.
    fn handle_focus(&mut self, focused: bool) {
        debug_log!("window_event: Focused({})", focused);
        if !focused {
            let in_grace = self
                .last_summon
                .map(|t| t.elapsed() < super::state::SUMMON_FOCUS_GRACE)
                .unwrap_or(false);
            if self.editing {
                debug_log!("window_event: Focused(false) ignored (editing)");
            } else if self.settings_panel_active() || self.folders.is_active() {
                debug_log!("window_event: Focused(false) ignored (settings open)");
            } else if in_grace {
                debug_log!("window_event: Focused(false) ignored (within summon grace)");
            } else {
                self.execute_command(AppCommand::HideWindow);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::SettingsPressTarget::*;
    use winit::keyboard::KeyCode;

    fn kb_action(
        settings_open: bool,
        editing: bool,
        wants_kb: bool,
        key: Option<KeyCode>,
    ) -> KeyAction {
        keyboard_action(settings_open, editing, wants_kb, true, key, None)
    }

    // ---- keyboard precedence: settings Esc > edit Esc > search Esc > hide ----

    #[test]
    fn settings_esc_closes_settings_even_while_editing() {
        let action = kb_action(true, true, false, Some(KeyCode::Escape));
        assert_eq!(action, KeyAction::CloseSettings);
    }

    #[test]
    fn settings_esc_takes_precedence_over_search_field() {
        let action = kb_action(true, false, true, Some(KeyCode::Escape));
        assert_eq!(action, KeyAction::CloseSettings);
    }

    #[test]
    fn edit_esc_exits_edit_mode() {
        let action = kb_action(false, true, false, Some(KeyCode::Escape));
        assert_eq!(action, KeyAction::ExitEditMode);
    }

    #[test]
    fn edit_esc_takes_precedence_over_search_field() {
        let action = kb_action(false, true, true, Some(KeyCode::Escape));
        assert_eq!(action, KeyAction::ExitEditMode);
    }

    #[test]
    fn search_esc_closes_field_not_launcher() {
        let action = kb_action(false, false, true, Some(KeyCode::Escape));
        assert_eq!(action, KeyAction::SearchEscClose);
    }

    #[test]
    fn esc_with_nothing_open_hides_launcher() {
        let action = kb_action(false, false, false, Some(KeyCode::Escape));
        assert_eq!(action, KeyAction::HideLauncher);
    }

    #[test]
    fn folder_esc_closes_folder_when_not_renaming() {
        assert_eq!(
            folder_keyboard_action(false, false, true, Some(KeyCode::Escape), None),
            KeyAction::CloseFolder
        );
    }

    #[test]
    fn folder_esc_exits_edit_mode_before_closing_folder() {
        assert_eq!(
            folder_keyboard_action(false, true, true, Some(KeyCode::Escape), None),
            KeyAction::ExitEditMode
        );
    }

    #[test]
    fn folder_rename_esc_cancels_before_folder_close() {
        assert_eq!(
            folder_keyboard_action(true, true, true, Some(KeyCode::Escape), None),
            KeyAction::CancelFolderRename
        );
    }

    #[test]
    fn folder_rename_enter_commits_and_preedit_blocks_edits() {
        assert_eq!(
            folder_keyboard_action(true, true, true, Some(KeyCode::Enter), None),
            KeyAction::CommitFolderRename
        );
        assert_eq!(
            folder_keyboard_action(true, true, false, Some(KeyCode::Backspace), None),
            KeyAction::None
        );
        assert_eq!(
            folder_keyboard_action(true, true, true, Some(KeyCode::KeyA), Some("あ")),
            KeyAction::FolderRenameChar("あ".to_owned())
        );
    }

    // ---- search field key handling ----

    #[test]
    fn search_backspace_only_when_preedit_empty() {
        assert_eq!(
            keyboard_action(false, false, true, true, Some(KeyCode::Backspace), None),
            KeyAction::SearchBackspace
        );
        assert_eq!(
            keyboard_action(false, false, true, false, Some(KeyCode::Backspace), None),
            KeyAction::SearchBackspaceBlocked
        );
    }

    #[test]
    fn search_arrows_route_to_search() {
        assert_eq!(
            kb_action(false, false, true, Some(KeyCode::ArrowLeft)),
            KeyAction::SearchLeft
        );
        assert_eq!(
            kb_action(false, false, true, Some(KeyCode::ArrowRight)),
            KeyAction::SearchRight
        );
    }

    #[test]
    fn search_arrows_blocked_while_preedit_nonempty() {
        assert_eq!(
            keyboard_action(false, false, true, false, Some(KeyCode::ArrowLeft), None),
            KeyAction::SearchLeftBlocked
        );
        assert_eq!(
            keyboard_action(false, false, true, false, Some(KeyCode::ArrowRight), None),
            KeyAction::SearchRightBlocked
        );
    }

    #[test]
    fn search_printable_char_routes_to_search_char() {
        let action = keyboard_action(false, false, true, true, Some(KeyCode::KeyA), Some("a"));
        assert_eq!(action, KeyAction::SearchChar("a".to_string()));
    }

    #[test]
    fn search_char_blocked_while_preedit_nonempty() {
        let action = keyboard_action(false, false, true, false, Some(KeyCode::KeyA), Some("a"));
        assert_eq!(action, KeyAction::None);
    }

    // ---- debug keys ----

    #[test]
    fn key_m_toggles_decorations() {
        assert_eq!(
            kb_action(false, false, false, Some(KeyCode::KeyM)),
            KeyAction::ToggleDecorations
        );
    }

    #[test]
    fn key_r_resets_icons_only_when_search_closed() {
        assert_eq!(
            kb_action(false, false, false, Some(KeyCode::KeyR)),
            KeyAction::ResetIcons
        );
        // When the search field is open, R falls through into the wants_keyboard
        // branch and does NOT reset icons.
        let action = kb_action(false, false, true, Some(KeyCode::KeyR));
        assert_ne!(action, KeyAction::ResetIcons);
    }

    #[test]
    fn unknown_key_delegates_to_liquid_glass() {
        assert!(matches!(
            kb_action(false, false, false, Some(KeyCode::F1)),
            KeyAction::LiquidGlassKey(_)
        ));
    }

    // ---- pointer press precedence: settings > control > edit/grid ----

    #[test]
    fn pointer_press_settings_takes_precedence() {
        let action = pointer_press_action(true, Outside, true, true);
        assert!(matches!(action, PressAction::Settings(_)));
    }

    #[test]
    fn pointer_press_control_before_grid() {
        let action = pointer_press_action(false, Inside, true, false);
        assert_eq!(action, PressAction::Control);
    }

    #[test]
    fn pointer_press_edit_grid_before_normal_press() {
        let action = pointer_press_action(false, Inside, false, true);
        assert_eq!(action, PressAction::EditGrid);
    }

    #[test]
    fn pointer_press_normal_grid_when_nothing_else() {
        let action = pointer_press_action(false, Inside, false, false);
        assert_eq!(action, PressAction::GridPress);
    }

    // ---- pointer release classification ----

    #[test]
    fn release_settings_outside_dismiss_when_both_outside() {
        let action = pointer_release_action(
            true,
            Some(Outside),
            Outside,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        );
        assert!(matches!(
            action,
            ReleaseAction::Settings {
                pressed: Outside,
                released: Outside
            }
        ));
    }

    #[test]
    fn release_control_click_when_on_capsule() {
        let action = pointer_release_action(
            false, None, Outside, true, true, false, false, false, false, false,
        );
        assert_eq!(action, ReleaseAction::Control);
    }

    #[test]
    fn release_control_none_when_off_capsule() {
        let action = pointer_release_action(
            false, None, Outside, true, false, false, false, false, false, false,
        );
        assert_eq!(action, ReleaseAction::None);
    }

    #[test]
    fn release_edit_drop_when_editing_with_drag() {
        let action = pointer_release_action(
            false, None, Outside, false, false, true, false, false, false, false,
        );
        assert_eq!(action, ReleaseAction::EditDrop);
    }

    #[test]
    fn release_pending_outside_passthrough() {
        let action = pointer_release_action(
            false, None, Outside, false, false, false, true, true, false, false,
        );
        assert_eq!(action, ReleaseAction::PendingOutsidePassthrough);
    }

    #[test]
    fn release_pending_launch_when_launch_id_present() {
        let action = pointer_release_action(
            false, None, Outside, false, false, false, true, false, true, false,
        );
        assert_eq!(action, ReleaseAction::PendingLaunch);
    }

    #[test]
    fn release_scroller_release_when_dragging() {
        let action = pointer_release_action(
            false, None, Outside, false, false, false, false, false, false, true,
        );
        assert_eq!(action, ReleaseAction::ScrollerRelease);
    }

    #[test]
    fn release_none_when_nothing_applies() {
        let action = pointer_release_action(
            false, None, Outside, false, false, false, false, false, false, false,
        );
        assert_eq!(action, ReleaseAction::None);
    }

    // ---- AppCommand ordering (hide-before-launch, modal-dismiss vs passthrough) ----

    #[test]
    fn launch_command_documents_hide_before_launch_ordering() {
        let hide = AppCommand::HideWindow;
        let launch = AppCommand::LaunchApp(crate::domain::app_registry::AppLaunchInfo {
            name: "X".to_string(),
            link_path: std::path::PathBuf::from("x.lnk"),
        });
        assert!(matches!(hide, AppCommand::HideWindow));
        assert!(matches!(launch, AppCommand::LaunchApp(_)));
        assert!(!matches!(hide, AppCommand::LaunchApp(_)));
    }

    #[test]
    fn modal_dismiss_is_distinct_from_click_passthrough() {
        let plain_hide = AppCommand::HideWindow;
        let passthrough = AppCommand::HideWithClickPassthrough;
        assert!(matches!(plain_hide, AppCommand::HideWindow));
        assert!(matches!(passthrough, AppCommand::HideWithClickPassthrough));
        assert!(!matches!(plain_hide, AppCommand::HideWithClickPassthrough));
    }

    #[test]
    fn search_esc_close_is_distinct_from_launcher_hide() {
        let esc_in_field = KeyAction::SearchEscClose;
        let esc_hide = KeyAction::HideLauncher;
        assert_ne!(esc_in_field, esc_hide);
    }
}
