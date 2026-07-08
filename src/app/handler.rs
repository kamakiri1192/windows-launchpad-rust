//! `ApplicationHandler<UserEvent>` implementation: a thin dispatcher that routes
//! raw events through `input` → `update` → `command` → `frame`.

use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

use crate::bottom_control;
use crate::debug_log;
use crate::features;
use crate::grid;
use crate::launch;
use crate::layout;
use crate::liquid_glass;
use crate::renderer::Renderer;
use crate::scroll::{Phase, Scroller};
use crate::startup_timer::prefix;
use crate::text;

use super::event::UserEvent;
use super::state::{
    App, SettingsPressTarget, INITIAL_WINDOW_HEIGHT, INITIAL_WINDOW_WIDTH, MIN_WINDOW_HEIGHT,
    MIN_WINDOW_WIDTH, SUMMON_FOCUS_GRACE,
};

use crate::{initial_window_position, load_window_icon};

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::BackdropFrameArrived => {
                self.request_redraw();
            }
            // All worker/watcher traffic arrives via the shared inbox; these
            // events are just wakeups. Drain here on the UI thread.
            UserEvent::InboxWakeup
            | UserEvent::IconLoaded { .. }
            | UserEvent::IconFailed { .. }
            | UserEvent::AppListDiff(_) => {
                self.drain_inbox();
            }
            UserEvent::Summon => {
                debug_log!("user_event: Summon received (visible={})", self.visible);
                self.summon();
            }
            UserEvent::QuitRequested => {
                debug_log!("user_event: QuitRequested received → process::exit(0)");
                // Force-exit the process. We previously used event_loop.exit(),
                // but the debug log showed the os-integration thread (tray +
                // hook) kept the process alive for >1.8s after the call — the
                // tray was still clickable, so 'Quit' appeared to need two
                // clicks. A hard exit terminates all threads immediately; the
                // OS releases the LL hook and removes the tray icon on process
                // teardown, so no manual cleanup is needed.
                std::process::exit(0);
            }
            UserEvent::ToggleSettings => {
                // Tray "Settings": ensure the window is visible first so the
                // overlay is actually shown.
                if !self.visible {
                    self.summon();
                }
                self.toggle_settings();
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        self.timer.mark(prefix::STARTUP, "window creation");
        let mut attrs = Window::default_attributes()
            .with_title("Launchpad")
            .with_transparent(true)
            // Drop the classic HWND back buffer (WS_EX_NOREDIRECTIONBITMAP) so
            // the DWM composites only our DirectComposition swap chain. Without
            // this, alpha=0 pixels are filled with the window's white
            // background brush and transparency reads as solid white.
            .with_no_redirection_bitmap(true)
            // Borderless: the glass tiles own the visuals, so we drop the OS
            // title bar / frame. Closing via Esc/Alt-F4.
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(
                INITIAL_WINDOW_WIDTH,
                INITIAL_WINDOW_HEIGHT,
            ))
            .with_min_inner_size(LogicalSize::new(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));

        if let Some(icon) = load_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        if let Some(position) = initial_window_position(event_loop) {
            attrs = attrs.with_position(position);
        }

        let window = event_loop.create_window(attrs).expect("create window");
        #[cfg(windows)]
        {
            if std::env::var_os("LAUNCHPAD_ALLOW_SCREENSHOT").is_some() {
                eprintln!("capture exclusion skipped: LAUNCHPAD_ALLOW_SCREENSHOT is set");
            } else {
                let exclusion = liquid_glass::windows_capture::exclude_window_from_capture(&window);
                if exclusion.attempted && !exclusion.success {
                    eprintln!("capture exclusion failed: {}", exclusion.message);
                } else if exclusion.attempted {
                    eprintln!("capture exclusion: {}", exclusion.message);
                }
            }
        }
        self.scale_factor = window.scale_factor() as f32;
        let (w, _h) = (window.inner_size().width, window.inner_size().height);
        self.layout = grid::GridLayout::default()
            .with_scale_factor(self.scale_factor)
            .centered(w as f32);

        let renderer = pollster::block_on(Renderer::new(
            window,
            &self.layout,
            self.event_proxy.clone(),
        ))
        .expect("init renderer");
        self.timer.mark(prefix::STARTUP, "renderer initialization");
        let bounds = self.layout.bounds(w as f32);
        let scroller = Scroller::new(bounds);
        let text = text::TextRenderer::new();

        self.renderer = Some(renderer);
        self.scroller = Some(scroller);
        self.text = Some(text);

        // First paint: empty/loading state, NO icon extraction. This is the
        // core Phase-1 win — the window is visible before any Shell/GDI work.
        self.relayout();
        self.request_redraw();
        self.timer.mark(prefix::STARTUP, "first redraw requested");
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                // Borderless window has no close button in normal use, but
                // Alt+F4 still reaches here. Treat it as "hide" rather than
                // "quit" so the launcher stays resident; real quit is via the
                // tray menu.
                self.hide();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let key_code = match event.physical_key {
                    winit::keyboard::PhysicalKey::Code(code) => Some(code),
                    winit::keyboard::PhysicalKey::Unidentified(_) => None,
                };

                // The settings overlay takes precedence over everything: Esc
                // closes it (doesn't hide the launcher), mirroring how edit mode
                // and the search field swallow Esc rather than quitting.
                if self.settings_open && key_code == Some(winit::keyboard::KeyCode::Escape) {
                    self.close_settings();
                    return;
                }

                // Edit mode takes precedence over everything except the search
                // field: Esc exits edit mode (doesn't hide), Enter/Done would
                // too. This branch sits before `wants_keyboard` so an open
                // search field still defers to edit-mode Esc.
                if self.editing && key_code == Some(winit::keyboard::KeyCode::Escape) {
                    self.exit_edit_mode();
                    return;
                }

                // While the search field has focus, the control eats most keys.
                if self.control.wants_keyboard() {
                    let handled = match key_code {
                        Some(winit::keyboard::KeyCode::Escape) => {
                            let c = self.control.wants_keyboard();
                            // If the field was open, Esc clears search and
                            // closes it instead of hiding the launcher.
                            if c {
                                self.control.press_close();
                                self.search_input_changed();
                                return;
                            }
                            false
                        }
                        Some(winit::keyboard::KeyCode::Backspace) => {
                            if self.control.preedit.is_empty() {
                                self.control.handle_backspace();
                                self.search_input_changed();
                            } else {
                                self.request_redraw();
                            }
                            true
                        }
                        Some(winit::keyboard::KeyCode::ArrowLeft) => {
                            if self.control.preedit.is_empty() {
                                self.control.handle_left();
                            }
                            self.request_redraw();
                            true
                        }
                        Some(winit::keyboard::KeyCode::ArrowRight) => {
                            if self.control.preedit.is_empty() {
                                self.control.handle_right();
                            }
                            self.request_redraw();
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return;
                    }
                    // Otherwise, let printable text through (typed below).
                    // Direct (non-IME) printable characters arrive in event.text.
                    if self.control.preedit.is_empty() {
                        if let Some(text) = &event.text {
                            if self.control.wants_keyboard() {
                                let mut any = false;
                                for ch in text.chars() {
                                    if self.control.handle_char(ch) {
                                        any = true;
                                    }
                                }
                                if any {
                                    self.search_input_changed();
                                    return;
                                }
                            }
                        }
                    }
                }

                if key_code == Some(winit::keyboard::KeyCode::Escape) {
                    // Esc with no open field: hide the launcher (stay resident).
                    self.hide();
                    return;
                }

                // M toggles the OS window frame on/off for easier debugging
                // (grab edges to resize, title bar to move) without rebuilding.
                if key_code == Some(winit::keyboard::KeyCode::KeyM) {
                    if let Some(r) = self.renderer.as_mut() {
                        r.toggle_decorations();
                        self.request_redraw();
                    }
                    return;
                }

                // R clears the icon cache and re-extracts every icon live, so
                // you can recover from a corrupted cache without restarting.
                if key_code == Some(winit::keyboard::KeyCode::KeyR)
                    && !self.control.wants_keyboard()
                {
                    self.reset_icons();
                    return;
                }

                if let (Some(r), Some(key_code)) = (self.renderer.as_mut(), key_code) {
                    if r.handle_liquid_glass_key(key_code) {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::Ime(event) => {
                use winit::event::Ime;
                if self.control.wants_keyboard() {
                    match event {
                        Ime::Preedit(s, _) => {
                            // Show the in-flight composition inline.
                            self.control.set_preedit(s);
                            self.search_input_changed();
                        }
                        Ime::Commit(text) => {
                            // IME commit: finalize the composition into the query.
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
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size.width, new_size.height);
                }
                self.relayout();
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                self.relayout();
                self.request_redraw();
            }
            WindowEvent::Moved(_) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.notify_window_moved();
                }
                self.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                // If the button is still down we treat leaving as a release.
                let dragging = self
                    .scroller
                    .as_ref()
                    .map(|s| s.phase == Phase::Dragging)
                    .unwrap_or(false);
                if dragging {
                    self.handle_drag_end();
                }
                // An edit-mode drag whose pointer leaves the window is finalized
                // where it last was (iOS keeps the icon where you let go).
                if self.editing && self.drag_app.is_some() {
                    self.commit_reorder();
                    self.drag_app = None;
                    self.relayout();
                }
                // A pending long-press is cancelled when the pointer leaves.
                self.pending_press = None;
                // Drop a pending control press if the pointer leaves.
                self.pressed_on_control = false;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_phys_x = position.x as f32;
                self.pointer_phys_y = position.y as f32;
                // Edit-mode drag: follow the pointer and live-reorder.
                if self.editing && self.drag_app.is_some() {
                    self.handle_edit_drag_move();
                    return;
                }
                // A pending press may promote to a real scroll drag once it
                // moves past slop. If it does, the scroller is now Dragging and
                // the move has already been applied.
                if self.pending_press.is_some() && self.maybe_promote_press_to_drag() {
                    return;
                }
                let dragging = self
                    .scroller
                    .as_ref()
                    .map(|s| s.phase == Phase::Dragging)
                    .unwrap_or(false);
                if dragging {
                    self.handle_drag_move(position.x as f32);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        // While the settings overlay is open, presses are
                        // consumed by the overlay: an inside-panel click may
                        // hit the close button, an outside click closes the
                        // overlay. No grid interaction is possible underneath.
                        if self.settings_open {
                            self.pressed_on_settings = Some(
                                self.settings_hit_target(self.pointer_phys_x, self.pointer_phys_y),
                            );
                            // Always swallow: outside clicks are handled on
                            // release (close), inside clicks may hit the ×.
                            return;
                        }
                        // If the press starts on the control capsule (or, in
                        // edit mode, the adjacent settings gear capsule), mark
                        // it so the release is treated as a control click and
                        // NOT as a scroll drag. The hit classification comes
                        // from the layout layer's hit map so render geometry
                        // and pointer targets share one calculation.
                        let intent =
                            self.bottom_control_intent(self.pointer_phys_x, self.pointer_phys_y);
                        let over_control = !matches!(
                            intent,
                            layout::bottom_control::BottomControlPointerIntent::None
                        );
                        self.pressed_on_control = over_control;
                        if over_control {
                            return;
                        }
                        // Edit mode: clicking an icon lifts it into a drag;
                        // clicking its ✕ badge hides it; clicking empty space
                        // (inside or outside the frame) exits edit mode. The
                        // classification (badge > drag > exit) comes from
                        // [`features::edit_mode::edit_press_classify`]; this
                        // branch runs the side effects (hide / drag start /
                        // exit).
                        //
                        // Note: the historical code used `app_index_at_pointer`
                        // (which returns `None` for both empty-in-frame *and*
                        // outside-frame, because `hit_test_app` clips to the
                        // frame) and exited edit mode in both cases. The
                        // classifier preserves that: `EmptyExit` covers
                        // empty-in-frame, `Noop` covers outside-frame, and both
                        // exit here.
                        if self.editing {
                            let px = self.pointer_phys_x;
                            let py = self.pointer_phys_y;
                            let hit = self.grid_hit_at_pointer(px, py);
                            let badge_hit = matches!(hit, layout::grid::GridHit::App(idx) if self.badge_hit(idx, px, py));
                            let intent = features::edit_mode::edit_press_classify(hit, badge_hit);
                            match intent {
                                features::edit_mode::EditPressIntent::HideApp { visible_index } => {
                                    let id = self.visible_app_ids()[visible_index].clone();
                                    debug_log!("edit-drag: badge press idx={visible_index}");
                                    self.hide_app(&id);
                                }
                                features::edit_mode::EditPressIntent::DragApp { visible_index } => {
                                    debug_log!("edit-drag: press idx={visible_index}");
                                    let id = self.visible_app_ids()[visible_index].clone();
                                    self.drag_app = Some(id);
                                    self.drag_x = px;
                                    self.drag_y = py;
                                    self.relayout();
                                    self.request_redraw();
                                }
                                features::edit_mode::EditPressIntent::EmptyExit
                                | features::edit_mode::EditPressIntent::Noop => {
                                    // Empty space (inside or outside the frame)
                                    // → exit edit mode (and persist).
                                    self.exit_edit_mode();
                                }
                            }
                            return;
                        }
                        // Normal mode: defer the scroll drag until the gesture
                        // resolves (drag past slop, or quick release, or long-
                        // press into edit mode).
                        self.begin_grid_press(Instant::now());
                    }
                    ElementState::Released => {
                        // Settings overlay open: handle close-button + outside
                        // clicks. Nothing underneath is reachable.
                        if self.settings_open {
                            let pressed = self.pressed_on_settings.take();
                            let px = self.pointer_phys_x;
                            let py = self.pointer_phys_y;
                            let released = self.settings_hit_target(px, py);
                            if pressed == Some(SettingsPressTarget::Outside)
                                && released == SettingsPressTarget::Outside
                            {
                                self.close_settings();
                                return;
                            }
                            if pressed == Some(released) {
                                self.handle_settings_click(released);
                            } else if pressed == Some(SettingsPressTarget::Outside)
                                && released == SettingsPressTarget::Outside
                            {
                                // Outside the panel → dismiss (like a modal).
                                self.close_settings();
                            }
                            return;
                        }
                        if self.pressed_on_control {
                            self.pressed_on_control = false;
                            // Only count as a click if it stayed on the
                            // capsule body. This mirrors the previous
                            // behavior, which re-tested only the main capsule
                            // shape (`hit_test_scaled`) on release — not the
                            // gear — so a press that landed on the gear but
                            // drifted off the capsule is dropped. The gear is
                            // re-resolved inside `handle_control_click`.
                            //
                            // We test the capsule shape directly (not the hit
                            // map) so that the edit-mode gear, which overlaps
                            // the capsule's right edge, stays reachable through
                            // the capsule region it shares — matching the
                            // previous `hit_test_scaled` behavior exactly.
                            let on_capsule = self.bottom_control_capsule_hit(
                                self.pointer_phys_x,
                                self.pointer_phys_y,
                            );
                            if on_capsule {
                                self.handle_control_click(self.pointer_phys_x, self.pointer_phys_y);
                            }
                            return;
                        }
                        // Edit-mode drag release: drop the icon here and persist.
                        if self.editing && self.drag_app.is_some() {
                            self.commit_reorder();
                            self.drag_app = None;
                            self.relayout();
                            self.request_redraw();
                            return;
                        }
                        // A pending press that released without dragging and
                        // without a long-press is a click → launch the app.
                        if let Some(press) = self.pending_press.take() {
                            if press
                                .is_outside_glass_click(self.pointer_phys_x, self.pointer_phys_y)
                            {
                                self.hide_with_click_passthrough();
                                return;
                            }
                            if let Some(app) = press
                                .launch_id(self.pointer_phys_x, self.pointer_phys_y)
                                .and_then(|id| self.registry.launch_info(id))
                            {
                                let link_path = app.link_path.clone();
                                let name = app.name.clone();
                                self.hide();
                                match launch::open_shortcut(&link_path) {
                                    Ok(()) => eprintln!("launched {}", name),
                                    Err(err) => eprintln!(
                                        "failed to launch {} ({}): {}",
                                        name,
                                        link_path.display(),
                                        err
                                    ),
                                }
                            }
                            return;
                        }
                        if let Some(app) = self.handle_pointer_release() {
                            // Dismiss first, launch second. `hide()` hands the
                            // hide straight to the DWM (a few ms) and resets the
                            // UI, while `ShellExecuteW` resolves the shortcut and
                            // spawns the target (tens to hundreds of ms). Doing
                            // them in this order makes the launcher feel like it
                            // vanishes the instant you click, instead of freezing
                            // on screen until the target app starts.
                            let link_path = app.link_path.clone();
                            let name = app.name.clone();
                            self.hide();
                            // NOTE: no `event_loop.exit()` — we stay resident
                            // so the next hot key can summon us instantly.
                            match launch::open_shortcut(&link_path) {
                                Ok(()) => eprintln!("launched {}", name),
                                Err(err) => eprintln!(
                                    "failed to launch {} ({}): {}",
                                    name,
                                    link_path.display(),
                                    err
                                ),
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.tick_frame();
            }
            WindowEvent::Focused(focused) => {
                debug_log!("window_event: Focused({})", focused);
                // Auto-hide when the launcher loses focus (clicking another
                // window, Alt-Tab, …). This is the macOS-Launchpad / Run-dialog
                // behavior. `hide()` is idempotent so the focus-loss that fires
                // right after we hide to launch an app is a harmless no-op.
                //
                // BUT: ignore a focus loss that happens within
                // SUMMON_FOCUS_GRACE of a summon. SetForegroundWindow can
                // briefly drop and re-acquire focus as the OS shuffles
                // windows, and without this guard the just-summoned launcher
                // would vanish within ~75ms on some machines.
                if !focused {
                    let in_grace = self
                        .last_summon
                        .map(|t| t.elapsed() < SUMMON_FOCUS_GRACE)
                        .unwrap_or(false);
                    // While editing we don't auto-hide on focus loss: clicking
                    // outside the launcher to dismiss edit mode would itself
                    // blur the window, and we want to exit edit mode cleanly
                    // (persisting the reorder) rather than vanish mid-edit.
                    // The settings overlay gets the same treatment so it isn't
                    // dismissed by a momentary focus shuffle.
                    if self.editing {
                        debug_log!("window_event: Focused(false) ignored (editing)");
                    } else if self.settings_panel_active() {
                        debug_log!("window_event: Focused(false) ignored (settings open)");
                    } else if in_grace {
                        debug_log!("window_event: Focused(false) ignored (within summon grace)");
                    } else {
                        self.hide();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Real quit path: the tray "Quit" command set the flag; now that the
        // current event is fully handled we can terminate the loop.
        if self.should_quit {
            event_loop.exit();
            return;
        }

        // Keep the loop pumping while the scroller or the bottom control is
        // animating; otherwise winit blocks until the next input or WGC
        // FrameArrived user event.
        let scroller_animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        let control_animating = self.control.mode.is_morphing()
            || matches!(self.control.mode, bottom_control::Mode::Indicator)
            || matches!(self.control.mode, bottom_control::Mode::Field);

        // Long-press timer: if a press is still pending, keep redrawing so we
        // notice when it crosses LONG_PRESS_THRESHOLD and enter edit mode.
        let long_press_pending = self.pending_press.is_some();
        if long_press_pending {
            self.maybe_long_press_into_edit(Instant::now());
        }

        // Edit mode keeps redrawing so the wiggle animation advances and the
        // dragged tile tracks the pointer smoothly.
        if scroller_animating || control_animating || self.editing || long_press_pending {
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}
