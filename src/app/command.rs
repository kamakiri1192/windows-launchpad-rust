//! Command execution at the app boundary.
//!
//! Side effects requested as [`super::event::AppCommand`] values (or
//! edit-mode [`EditModeCommand`][edit] values) are executed here. The update
//! and frame layers call these methods; they are the single place that touches
//! window visibility, the OS hotkey/tray adapter (via `platform_windows`), app
//! launching, and persistence stores.
//!
//! [edit]: crate::features::edit_mode::EditModeCommand
//!
//! The app shell is intentionally not a pure reducer: these are `&mut self`
//! methods that run eagerly, preserving the historical side-effect ordering
//! (hide before launch, modal dismiss without passthrough, etc.).

use std::time::Instant;

use crate::debug_log;
use crate::features::edit_mode::EditModeCommand;
use crate::scroll::Phase;

use super::event::AppCommand;
use super::state::App;

impl App {
    /// Load the persisted user customization (Phase 7 launcher layout: item
    /// order, folders, hidden apps) into `launcher_state`. Called once at
    /// startup, before the first scan is ingested, so apps are placed in the
    /// user's arrangement from the first frame.
    ///
    /// Migration: if the Phase 7 `launcher_state` key is present it is used
    /// directly. Otherwise the legacy `app_order` + `hidden_ids` binary keys
    /// are read and converted via [`LauncherState::from_legacy`]. A missing or
    /// corrupt store is a no-op (state stays empty / non-customized), so a bad
    /// blob never blocks startup or wipes other settings.
    pub(crate) fn load_customization(&mut self) {
        self.settings = self.cache.get_settings();
        if let Some(state) = self.cache.get_launcher_state() {
            self.launcher_state = state;
            return;
        }
        // Legacy migration path: convert the old binary app_order + hidden_ids
        // keys into the item-based launcher state.
        let order = self.cache.get_app_order();
        let hidden = self.cache.get_hidden_ids();
        if !order.is_empty() || !hidden.is_empty() {
            self.launcher_state =
                crate::domain::launcher_state::LauncherState::from_legacy(order, hidden);
        }
    }

    /// Persist the current launcher layout so it survives across launches.
    /// Called after a drag-to-reorder, hide/unhide, or folder change. Cheap:
    /// one small JSON blob upsert. Errors are logged but never panic the UI.
    pub(crate) fn persist_launcher_state(&self) {
        if self.qa_enabled() {
            return;
        }
        if let Err(e) = self.cache.put_launcher_state(&self.launcher_state) {
            eprintln!("layout: failed to persist launcher state: {e}");
        }
    }

    /// Persist the current display order so it survives across launches. Called
    /// after a drag-to-reorder completes (and on hide). Phase 7 routes this
    /// through the unified launcher state; this method is kept as the
    /// edit-mode command target so the command boundary stays stable.
    pub(crate) fn persist_user_order(&self) {
        self.persist_launcher_state();
    }

    /// Persist the current hidden-app list. Called after a hide/unhide change.
    /// Phase 7 routes this through the unified launcher state.
    pub(crate) fn persist_hidden(&self) {
        self.persist_launcher_state();
    }

    pub(crate) fn persist_settings(&self) {
        if self.qa_enabled() {
            return;
        }
        if let Err(e) = self.cache.put_settings(&self.settings) {
            eprintln!("settings: failed to persist settings: {e}");
        }
    }

    /// Hide the launcher window and reset transient UI state (search field,
    /// scroll position, IME), but keep the process + event loop alive so it
    /// can be summoned again. Idempotent: a no-op if already hidden.
    pub(crate) fn hide(&mut self) {
        if !self.visible {
            debug_log!("hide: already hidden, no-op");
            return;
        }
        debug_log!("hide: hiding window");
        if let Some(r) = self.renderer.as_ref() {
            r.window.set_visible(false);
            r.window.set_ime_allowed(false);
        }
        // Exit edit mode if active, persisting any reorder before we vanish.
        if self.editing {
            self.exit_edit_mode();
        }
        // Close the settings overlay so a re-summon starts clean.
        self.settings_open = false;
        self.settings_panel_progress = 0.0;
        self.folders = crate::features::folders::FolderFeatureState::default();
        self.folder_layout = None;
        self.pending_press = None;
        // Drop any in-progress search / IME composition so the next summon
        // starts clean.
        self.control.press_close();
        self.relayout();
        // Reset scroll to page 0 so the next appearance doesn't land mid-page.
        if let Some(s) = self.scroller.as_mut() {
            s.position = 0.0;
            s.velocity = 0.0;
            s.phase = Phase::Idle;
        }
        self.last_page = 0;
        self.visible = false;
        self.request_redraw();
    }

    /// Hide the launcher after a transparent-area click and, on Windows, send
    /// a best-effort replacement click to whatever is now under the cursor.
    pub(crate) fn hide_with_click_passthrough(&mut self) {
        self.hide();
        #[cfg(windows)]
        {
            if crate::platform::windows::replay_left_click_at_cursor() {
                debug_log!("outside-click: replayed click to underlying window");
            } else {
                debug_log!("outside-click: failed to replay click to underlying window");
            }
        }
    }

    /// Show the launcher window and steal focus. Counterpart to [`hide`].
    /// Re-centers on the primary monitor so a multi-monitor move doesn't
    /// strand the launcher on the wrong screen.
    pub(crate) fn summon(&mut self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        debug_log!("summon: showing window (visible was {})", self.visible);
        r.window.set_visible(true);
        // Steal focus. focus_window() can be silently denied by Windows when
        // the foreground already belongs to another app (common after hide()),
        // so we also allow-set-foreground + re-assert focus. If it still fails
        // the user at least sees the window appear (visible=true above) even
        // if it's not topmost.
        #[cfg(windows)]
        {
            // ASFW_ANY (-1) lifts the SetForegroundWindow restriction so any
            // process (incl. ours) can come to the front. This is what lets a
            // hotkey-triggered summon reliably raise the window instead of
            // just flashing the taskbar after the window was hidden.
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
                const ASFW_ANY: u32 = u32::MAX; // -1 as the Win32 ASFW_ANY sentinel
                let _ = AllowSetForegroundWindow(ASFW_ANY);
            }
        }
        r.window.focus_window();
        self.visible = true;
        // Record the summon time so a focus-transition artifact in the next
        // SUMMON_FOCUS_GRACE is ignored instead of instantly hiding us.
        self.last_summon = Some(Instant::now());
        self.request_redraw();
        debug_log!("summon: window shown + focus requested");
    }

    /// Execute one [`AppCommand`] at the app boundary. Preserves the historical
    /// side-effect ordering: e.g. launch hides the window first and opens the
    /// shortcut second.
    pub(super) fn execute_command(&mut self, command: AppCommand) {
        match command {
            AppCommand::RequestRedraw => self.request_redraw(),
            AppCommand::HideWindow => self.hide(),
            AppCommand::HideWithClickPassthrough => self.hide_with_click_passthrough(),
            AppCommand::Summon => self.summon(),
            AppCommand::LaunchApp(info) => {
                let link_path = info.link_path.clone();
                let name = info.name.clone();
                self.hide();
                match crate::platform::launch::open_shortcut(&link_path) {
                    Ok(()) => eprintln!("launched {}", name),
                    Err(err) => eprintln!(
                        "failed to launch {} ({}): {}",
                        name,
                        link_path.display(),
                        err
                    ),
                }
            }
            AppCommand::PersistSettings => self.persist_settings(),
            AppCommand::PersistUserOrder => self.persist_user_order(),
            AppCommand::PersistHidden => self.persist_hidden(),
            AppCommand::Relayout => self.relayout(),
            AppCommand::ResetIconCache => self.reset_icons(),
            // Edit-mode-consolidated side effects:
            AppCommand::SetEditing(value) => self.set_editing(value),
            AppCommand::SetDragItem(value) => self.drag_item = value,
            AppCommand::SetDragPos(x, y) => {
                self.drag_x = x;
                self.drag_y = y;
            }
            AppCommand::ResetWigglePhase => self.wiggle_phase = 0.0,
            AppCommand::CancelScroll => {
                if let Some(s) = self.scroller.as_mut() {
                    if s.phase != Phase::Idle {
                        s.phase = Phase::Idle;
                        s.velocity = 0.0;
                    }
                }
            }
            AppCommand::ClearPendingPress => self.pending_press = None,
            AppCommand::SetSortManual => {
                self.settings.sort_order = crate::domain::settings::SortOrder::Manual;
            }
            AppCommand::HideApp(app_id) => self.hide_app(&app_id),
            AppCommand::SettleToPage(page) => {
                if let Some(s) = self.scroller.as_mut() {
                    if s.phase == Phase::Idle && s.settle_to_page(page) {
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Execute a batch of [`EditModeCommand`]s by projecting each onto the
    /// equivalent [`AppCommand`] and running it through [`execute_command`].
    ///
    /// This is the Phase 5 consolidation point: edit-mode feature logic returns
    /// `Vec<EditModeCommand>` (see [`crate::features::edit_mode`]) and the app
    /// shell runs the side effects here, so edit-mode, settings, search, and
    /// grid all share one command-execution boundary.
    ///
    /// The mapping is order-preserving: commands run in the order the feature
    /// module emitted them (e.g. `SetSortManual` before `PersistSettings`
    /// before `PersistUserOrder` on a commit), matching the historical inline
    /// `commit_reorder` sequence.
    pub(super) fn execute_edit_mode_commands(&mut self, commands: Vec<EditModeCommand>) {
        for cmd in commands {
            let app_cmd = match cmd {
                EditModeCommand::SetEditing(v) => AppCommand::SetEditing(v),
                EditModeCommand::SetDragItem(v) => AppCommand::SetDragItem(v),
                EditModeCommand::SetDragPos(x, y) => AppCommand::SetDragPos(x, y),
                EditModeCommand::ResetWigglePhase => AppCommand::ResetWigglePhase,
                EditModeCommand::CancelScroll => AppCommand::CancelScroll,
                EditModeCommand::ClearPendingPress => AppCommand::ClearPendingPress,
                EditModeCommand::Relayout => AppCommand::Relayout,
                EditModeCommand::RequestRedraw => AppCommand::RequestRedraw,
                EditModeCommand::PersistUserOrder => AppCommand::PersistUserOrder,
                EditModeCommand::PersistHidden => AppCommand::PersistHidden,
                EditModeCommand::PersistSettings => AppCommand::PersistSettings,
                EditModeCommand::SetSortManual => AppCommand::SetSortManual,
                EditModeCommand::HideApp(id) => AppCommand::HideApp(id),
                EditModeCommand::SettleToPage(page) => AppCommand::SettleToPage(page),
            };
            self.execute_command(app_cmd);
        }
    }

    /// `editing = value` (idempotent). Logs the first transition only. Exposed
    /// as a narrow helper so [`execute_command`] can apply `SetEditing` without
    /// reaching into the field directly; the log-on-first-transition behavior
    /// is preserved.
    fn set_editing(&mut self, value: bool) {
        let was_editing = self.editing;
        self.editing = value;
        if value && !was_editing {
            debug_log!("edit-mode: entered");
        } else if !value && was_editing {
            debug_log!("edit-mode: exited");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::app_id::AppId;
    use crate::domain::launcher_item::LauncherItem;
    use crate::features::edit_mode::EditModeCommand;

    /// The pure projection from `EditModeCommand` to `AppCommand` is total:
    /// every edit-mode variant maps to exactly one app command. This test
    /// pins the mapping so a future edit-mode variant cannot silently drop out
    /// of the command boundary (the compiler exhaustive-match already enforces
    /// this at the `match`, but the test documents the intended mapping).
    #[test]
    fn edit_mode_command_maps_exhaustively_and_order_preserving() {
        let id = AppId::from_normalized("app-a");
        let edit_cmds = vec![
            EditModeCommand::SetEditing(true),
            EditModeCommand::SetDragItem(Some(LauncherItem::App(id.clone()))),
            EditModeCommand::SetDragPos(10.0, 20.0),
            EditModeCommand::ResetWigglePhase,
            EditModeCommand::CancelScroll,
            EditModeCommand::ClearPendingPress,
            EditModeCommand::Relayout,
            EditModeCommand::RequestRedraw,
            EditModeCommand::PersistUserOrder,
            EditModeCommand::PersistHidden,
            EditModeCommand::PersistSettings,
            EditModeCommand::SetSortManual,
            EditModeCommand::HideApp(id.clone()),
            EditModeCommand::SettleToPage(2),
        ];
        let mapped: Vec<AppCommand> = edit_cmds
            .iter()
            .map(|c| match c {
                EditModeCommand::SetEditing(v) => AppCommand::SetEditing(*v),
                EditModeCommand::SetDragItem(v) => AppCommand::SetDragItem(v.clone()),
                EditModeCommand::SetDragPos(x, y) => AppCommand::SetDragPos(*x, *y),
                EditModeCommand::ResetWigglePhase => AppCommand::ResetWigglePhase,
                EditModeCommand::CancelScroll => AppCommand::CancelScroll,
                EditModeCommand::ClearPendingPress => AppCommand::ClearPendingPress,
                EditModeCommand::Relayout => AppCommand::Relayout,
                EditModeCommand::RequestRedraw => AppCommand::RequestRedraw,
                EditModeCommand::PersistUserOrder => AppCommand::PersistUserOrder,
                EditModeCommand::PersistHidden => AppCommand::PersistHidden,
                EditModeCommand::PersistSettings => AppCommand::PersistSettings,
                EditModeCommand::SetSortManual => AppCommand::SetSortManual,
                EditModeCommand::HideApp(i) => AppCommand::HideApp(i.clone()),
                EditModeCommand::SettleToPage(p) => AppCommand::SettleToPage(*p),
            })
            .collect();
        // The mapping is 1:1 and order-preserving.
        assert_eq!(mapped.len(), edit_cmds.len());
        assert!(matches!(mapped[0], AppCommand::SetEditing(true)));
        assert!(matches!(mapped[11], AppCommand::SetSortManual));
        assert!(matches!(mapped[12], AppCommand::HideApp(_)));
    }

    /// `commit_drag` emits `SetSortManual` *before* the persist commands so the
    /// persisted settings carry the new sort order. This is the historical
    /// `commit_reorder` sequence (`sort_order = Manual` → `persist_settings` →
    /// `persist_user_order`) and the Phase 5 consolidation must preserve it.
    #[test]
    fn commit_drag_command_order_is_sort_manual_before_persist() {
        use crate::features::edit_mode::{commit_drag, EditModeState};
        let mut state = EditModeState {
            editing: true,
            drag_item: Some(LauncherItem::App(AppId::from_normalized("dragged"))),
            ..EditModeState::default()
        };
        let cmds = commit_drag(&state);
        assert_eq!(
            cmds,
            vec![
                EditModeCommand::SetSortManual,
                EditModeCommand::PersistSettings,
                EditModeCommand::PersistUserOrder,
            ]
        );
        // No drag → no persist (the historical commit_reorder was only called
        // when a drag was in flight).
        state.drag_item = None;
        assert!(commit_drag(&state).is_empty());
    }

    /// `enter` emits the entry side effects in the historical order:
    /// SetEditing → ClearPendingPress → ResetWigglePhase → CancelScroll, then
    /// the optional app lift (SetDragApp + SetDragPos), then Relayout +
    /// RequestRedraw.
    #[test]
    fn enter_command_order_matches_historical_entry_sequence() {
        use crate::features::edit_mode::{enter, EditModeState, PointerSnapshot};
        let mut state = EditModeState::default();
        let visible = vec![AppId::from_normalized("a"), AppId::from_normalized("b")];
        let cmds = enter(
            &mut state,
            Some(1),
            &visible,
            PointerSnapshot::new(50.0, 60.0),
        );
        // The entry core always comes first, in this order.
        assert_eq!(cmds[0], EditModeCommand::SetEditing(true));
        assert_eq!(cmds[1], EditModeCommand::ClearPendingPress);
        assert_eq!(cmds[2], EditModeCommand::ResetWigglePhase);
        assert_eq!(cmds[3], EditModeCommand::CancelScroll);
        // Then the app lift.
        assert_eq!(
            cmds[4],
            EditModeCommand::SetDragItem(Some(LauncherItem::App(AppId::from_normalized("b"))))
        );
        assert_eq!(cmds[5], EditModeCommand::SetDragPos(50.0, 60.0));
        // Then relayout + redraw.
        assert_eq!(cmds[6], EditModeCommand::Relayout);
        assert_eq!(cmds[7], EditModeCommand::RequestRedraw);
        assert_eq!(cmds.len(), 8);
    }

    /// `exit` with an in-flight drag runs the commit commands *before* the
    /// exit transitions, so the drop is persisted before editing is cleared.
    #[test]
    fn exit_with_drag_runs_commit_before_clearing() {
        use crate::features::edit_mode::{commit_drag, exit, EditModeState};
        let mut state = EditModeState {
            editing: true,
            drag_item: Some(LauncherItem::App(AppId::from_normalized("dragged"))),
            ..EditModeState::default()
        };
        let commit = commit_drag(&state);
        let cmds = exit(&mut state, commit);
        // Commit (SetSortManual, PersistSettings, PersistUserOrder) first…
        assert_eq!(cmds[0], EditModeCommand::SetSortManual);
        assert_eq!(cmds[1], EditModeCommand::PersistSettings);
        assert_eq!(cmds[2], EditModeCommand::PersistUserOrder);
        // …then the exit transitions.
        assert_eq!(cmds[3], EditModeCommand::SetEditing(false));
        assert_eq!(cmds[4], EditModeCommand::SetDragItem(None));
        assert_eq!(cmds[5], EditModeCommand::ClearPendingPress);
        assert_eq!(cmds[6], EditModeCommand::Relayout);
        assert_eq!(cmds[7], EditModeCommand::RequestRedraw);
    }
}
