//! `ApplicationHandler<UserEvent>` implementation: a thin adapter that converts
//! raw winit events into [`AppAction`] values and dispatches them through
//! [`App::handle_action`].
//!
//! The handler no longer calls feature methods, platform adapters, or the
//! renderer inline. It:
//!
//! 1. classifies raw events using the pure functions from [`super::action`]
//!    (`keyboard_action`, `pointer_press_action`, `pointer_release_action`);
//! 2. wraps them into [`AppAction`];
//! 3. dispatches via [`App::handle_action`], which runs the state transition
//!    and side-effect commands.
//!
//! This is the production "raw event → AppAction → update → AppCommand →
//! command executor" path. Side effects (hide, launch, passthrough, persist,
//! reset) all flow through [`App::execute_command`].

use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
#[cfg(target_os = "macos")]
use winit::platform::macos::WindowAttributesExtMacOS;
#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;
#[cfg(target_os = "macos")]
use winit::window::WindowLevel;
use winit::window::{Window, WindowId};

use crate::debug_log;
use crate::grid;
#[cfg(windows)]
use crate::liquid_glass;
use crate::renderer::text_engine as text;
use crate::renderer::Renderer;
use crate::scroll::{Phase, Scroller};
use crate::startup_timer::prefix;

use super::action::{
    folder_keyboard_action, keyboard_action, pointer_press_action, pointer_release_action,
    AppAction, KeyAction, PressAction, ReleaseAction,
};
use super::event::UserEvent;
use super::state::{
    App, INITIAL_WINDOW_HEIGHT, INITIAL_WINDOW_WIDTH, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH,
};

use crate::{initial_window_position, load_window_icon};

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let action = match event {
            UserEvent::BackdropFrameArrived => AppAction::BackdropFrameArrived,
            UserEvent::InboxWakeup
            | UserEvent::IconLoaded { .. }
            | UserEvent::IconFailed { .. }
            | UserEvent::AppListDiff(_) => AppAction::DrainInbox,
            UserEvent::Summon => {
                debug_log!("user_event: Summon received (visible={})", self.visible);
                AppAction::Summon
            }
            UserEvent::QuitRequested => AppAction::QuitRequested,
            UserEvent::ToggleSettings => AppAction::ToggleSettings,
            UserEvent::NativeScroll(raw) => {
                let Some(sample) = self.scroll_sample_adapter.adapt_native(raw) else {
                    return;
                };
                AppAction::ScrollSample(sample)
            }
        };
        self.handle_action(action);
        self.publish_input_routing_snapshot();
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        self.timer.mark(prefix::STARTUP, "window creation");
        let mut attrs = Window::default_attributes()
            .with_title("Launchpad")
            .with_transparent(true)
            // Borderless: the glass tiles own the visuals, so we drop the OS
            // title bar / frame. Closing via Esc/Alt-F4.
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(
                INITIAL_WINDOW_WIDTH,
                INITIAL_WINDOW_HEIGHT,
            ))
            .with_min_inner_size(LogicalSize::new(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
        #[cfg(windows)]
        {
            // Drop the classic HWND back buffer so DWM composites only the
            // DirectComposition swap chain and preserves per-pixel alpha.
            attrs = attrs.with_no_redirection_bitmap(true);
        }
        #[cfg(target_os = "macos")]
        {
            attrs = attrs
                .with_titlebar_hidden(true)
                .with_titlebar_buttons_hidden(true)
                .with_fullsize_content_view(true)
                .with_has_shadow(false)
                .with_accepts_first_mouse(true);
            if std::env::var_os("LAUNCHPAD_PROFILE_KEEP_VISIBLE").as_deref()
                == Some(std::ffi::OsStr::new("1"))
                || std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_some()
            {
                // Keep performance runs genuinely visible even while the
                // automation process samples logs in another app. Otherwise
                // Core Animation throttles an occluded CAMetalLayer and the
                // result measures window occlusion instead of rendering. The
                // native input QA window likewise must stay directly above its
                // passive probe so target resolution is deterministic.
                attrs = attrs.with_window_level(WindowLevel::AlwaysOnTop);
            }
        }
        if let Some(viewport) = self.qa_runner.as_ref().map(|runner| runner.viewport()) {
            attrs = attrs
                .with_visible(false)
                // The production minimum is expressed in logical points. On
                // Retina displays it can exceed a physically-sized QA target
                // (for example 480pt becomes 960px), silently changing the
                // deterministic viewport.
                .with_min_inner_size(PhysicalSize::new(1, 1))
                .with_inner_size(PhysicalSize::new(viewport[0], viewport[1]));
            self.visible = false;
        } else if std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_some() {
            attrs = attrs
                .with_min_inner_size(PhysicalSize::new(1, 1))
                .with_inner_size(PhysicalSize::new(1000, 700))
                .with_position(PhysicalPosition::new(100, 100));
        }

        if let Some(icon) = load_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        if !self.qa_enabled()
            && std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_none()
        {
            if let Some(position) = initial_window_position(event_loop) {
                attrs = attrs.with_position(position);
            }
        }

        let window = event_loop.create_window(attrs).expect("create window");
        #[cfg(target_os = "macos")]
        {
            if !crate::platform::macos::integration::enable_window_mouse_events(&window) {
                eprintln!("input-routing: failed to enable macOS launcher mouse events");
            }
            self._macos_input =
                crate::platform::macos::input_passthrough::MacInputPassthrough::install(
                    &window,
                    self.input_routing_publisher.clone(),
                    self.event_proxy.clone(),
                    self.scroll_clock_origin,
                );
            if self._macos_input.is_none() {
                eprintln!("input-routing: failed to install macOS local event monitor");
            }
        }
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
            !self.qa_enabled(),
        ))
        .expect("init renderer");
        #[cfg(target_os = "macos")]
        {
            // Renderer initialization may block the event loop long enough for
            // AppKit's initial focus notification to become stale. Reassert
            // activation only after the window can process the resulting
            // events.
            crate::platform::macos::integration::activate_application();
            if std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_some() {
                let _ = crate::platform::macos::integration::order_window_front_for_qa(
                    &renderer.window,
                );
            }
            renderer.window.focus_window();
        }
        self.timer.mark(prefix::STARTUP, "renderer initialization");
        let bounds = self.layout.bounds(w as f32);
        let scroller = Scroller::new(bounds);
        let text = text::TextRenderer::new();

        self.renderer = Some(renderer);
        self.scroller = Some(scroller);
        self.text = Some(text);

        // Apply the persisted Liquid Glass parameters so the user's last
        // tuning survives a restart. Debug-only flags (overlays, disable
        // toggles, window decorations) are intentionally NOT restored: a
        // stale debug view must never survive a relaunch.
        self.apply_persisted_liquid_glass_to_renderer();

        // First paint: empty/loading state, NO icon extraction. This is the
        // core Phase-1 win — the window is visible before any Shell/GDI work.
        self.relayout();
        self.request_redraw();
        self.start_qa(Instant::now());
        self.timer.mark(prefix::STARTUP, "first redraw requested");
        self.publish_input_routing_snapshot();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let action = match event {
            WindowEvent::CloseRequested => AppAction::CloseRequested,
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let key_code = match event.physical_key {
                    winit::keyboard::PhysicalKey::Code(code) => Some(code),
                    winit::keyboard::PhysicalKey::Unidentified(_) => None,
                };
                let key_action = if self.context_menu.is_active() {
                    // ESC dismisses the context menu; any other key is swallowed
                    // while the menu is open.
                    if key_code == Some(winit::keyboard::KeyCode::Escape) {
                        self.close_context_menu();
                    }
                    KeyAction::None
                } else if self.folders.is_active() && !self.settings_open {
                    folder_keyboard_action(
                        self.folders.rename.is_some(),
                        self.editing,
                        self.folders
                            .rename
                            .as_ref()
                            .is_none_or(|editor| editor.preedit.is_empty()),
                        self.settings.debug_keys_enabled,
                        key_code,
                        event.text.as_deref(),
                    )
                } else {
                    keyboard_action(
                        self.settings_open,
                        self.editing,
                        self.control.wants_keyboard(),
                        self.control.preedit.is_empty(),
                        self.settings.debug_keys_enabled,
                        key_code,
                        event.text.as_deref(),
                    )
                };
                AppAction::Keyboard(key_action)
            }
            WindowEvent::Ime(ime) => AppAction::Ime(ime),
            WindowEvent::Resized(new_size) => AppAction::Resized {
                width: new_size.width,
                height: new_size.height,
            },
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                AppAction::ScaleFactorChanged { scale_factor }
            }
            WindowEvent::Moved(_) => {
                #[cfg(target_os = "macos")]
                if let Some(passthrough) = &self._macos_input {
                    if let Some(r) = &self.renderer {
                        passthrough.refresh_geometry(&r.window);
                    }
                }
                AppAction::Moved
            }
            WindowEvent::CursorLeft { .. } => AppAction::CursorLeft,
            WindowEvent::CursorMoved { position, .. } => AppAction::PointerMoved {
                x: position.x as f32,
                y: position.y as f32,
            },
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => crate::input_routing::PointerButton::Left,
                    MouseButton::Right => crate::input_routing::PointerButton::Right,
                    _ => return,
                };
                AppAction::PointerButton {
                    button,
                    pressed: state == ElementState::Pressed,
                }
            }
            WindowEvent::MouseWheel {
                delta,
                phase: touch_phase,
                ..
            } => {
                #[cfg(target_os = "macos")]
                if self._macos_input.is_some() {
                    // The AppKit monitor already queued this exact native
                    // packet with separate contact/momentum phases.
                    return;
                }
                let (px, source) = Self::wheel_delta_to_physical_px(delta, self.scale_factor);
                let collapsed_phase = match touch_phase {
                    winit::event::TouchPhase::Started => {
                        crate::input_routing::CollapsedScrollPhase::Started
                    }
                    winit::event::TouchPhase::Moved => {
                        crate::input_routing::CollapsedScrollPhase::Moved
                    }
                    winit::event::TouchPhase::Ended => {
                        crate::input_routing::CollapsedScrollPhase::Ended
                    }
                    winit::event::TouchPhase::Cancelled => {
                        crate::input_routing::CollapsedScrollPhase::Cancelled
                    }
                };
                #[cfg(target_os = "macos")]
                let direction_inverted_from_device = source
                    == crate::input_routing::ScrollSource::Precise
                    && crate::platform::macos::scroll::natural_scroll_enabled();
                #[cfg(not(target_os = "macos"))]
                let direction_inverted_from_device = false;
                let timestamp_us = Instant::now()
                    .saturating_duration_since(self.scroll_clock_origin)
                    .as_micros()
                    .min(u128::from(u64::MAX)) as u64;
                let Some(sample) = self.scroll_sample_adapter.adapt_collapsed(
                    timestamp_us,
                    px,
                    source,
                    collapsed_phase,
                    direction_inverted_from_device,
                    self.scale_factor,
                ) else {
                    return;
                };
                #[cfg(feature = "wheel-debug")]
                eprintln!(
                    "wheel-event phase={:?} raw=({:.3},{:.3}) canonical=({:.3},{:.3}) source={:?} contact={:?} momentum={:?} gesture_id={}",
                    touch_phase,
                    sample.raw_dx,
                    sample.raw_dy,
                    sample.canonical_dx,
                    sample.canonical_dy,
                    sample.source,
                    sample.contact_phase,
                    sample.momentum_phase,
                    sample.gesture_id,
                );
                AppAction::ScrollSample(sample)
            }
            WindowEvent::RedrawRequested => AppAction::RedrawRequested,
            WindowEvent::Focused(focused) => AppAction::Focused(focused),
            _ => return,
        };
        self.handle_action(action);
        self.publish_input_routing_snapshot();
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        _event: winit::event::DeviceEvent,
    ) {
        // DeviceEvent::MouseWheel was previously mirrored here for macOS
        // trackpads, but this caused double-delivery (both DeviceEvent and
        // WindowEvent fire). The WindowEvent path is now sufficient and
        // produces pixel-level deltas. This handler is intentionally a no-op.
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Real quit path: the tray "Quit" command set the flag; now that the
        // current event is fully handled we can terminate the loop.
        if self.should_quit {
            event_loop.exit();
            return;
        }

        // Dispatch a tick action (long-press check + animation-gated redraw).
        let now = Instant::now();
        self.handle_action(AppAction::Tick { now });
        self.publish_input_routing_snapshot();
        if self.qa_capture_due(now) {
            // Windows does not deliver RedrawRequested for a hidden window.
            // QA therefore advances the exact production frame path from its
            // own fixed-rate deadline while normal visible mode remains event
            // driven.
            self.tick_frame();
        }
        if self.qa_finished(now) {
            self.finalize_qa();
            event_loop.exit();
            return;
        }
        if let Some(deadline) = self.qa_next_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline.max(now)));
        } else {
            // Persistent platform capture sends BackdropFrameArrived whenever
            // the desktop content changes. Static desktops therefore stay
            // idle, while video/animation drives redraws at the stream cadence.
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl App {
    /// Translate winit wheel input into physical pixels. PixelDelta is already
    /// physical on winit 0.30; applying the scale factor again would make the
    /// paging distance DPI-dependent.
    fn wheel_delta_to_physical_px(
        delta: winit::event::MouseScrollDelta,
        scale_factor: f32,
    ) -> ((f32, f32), crate::input_routing::ScrollSource) {
        match delta {
            winit::event::MouseScrollDelta::LineDelta(x, y) => {
                let scale = scale_factor.max(0.1);
                let px_per_line = crate::layout::settings_panel::row_step(scale);
                (
                    (x * px_per_line, y * px_per_line),
                    crate::input_routing::ScrollSource::Line,
                )
            }
            winit::event::MouseScrollDelta::PixelDelta(px) => (
                (px.x as f32, px.y as f32),
                crate::input_routing::ScrollSource::Precise,
            ),
        }
    }

    /// Classify a left-button press into a [`PressAction`] using the current
    /// shell flags and the pointer position. This feeds
    /// [`AppAction::PointerPress`].
    pub(crate) fn classify_pointer_press(&self, px: f32, py: f32) -> PressAction {
        let settings_target = if self.settings_open {
            self.settings_hit_target(px, py)
        } else {
            super::state::SettingsPressTarget::Outside
        };
        let over_control = if self.settings_open {
            false
        } else {
            let intent = self.bottom_control_intent(px, py);
            !matches!(
                intent,
                crate::layout::bottom_control::BottomControlPointerIntent::None
            )
        };
        // The context menu takes priority over the folder panel so that,
        // while both are open, a click anywhere dismisses the menu without
        // also triggering the folder panel (e.g. closing the folder via its
        // dismiss backdrop).
        if self.context_menu.is_active() {
            return PressAction::ContextMenu;
        }
        if self.folders.is_active() && self.drag_item.is_none() && !(self.editing && over_control) {
            return PressAction::Folder;
        }
        pointer_press_action(
            self.settings_open,
            settings_target,
            over_control,
            self.editing,
        )
    }

    /// Classify a left-button release into a [`ReleaseAction`] using the current
    /// shell flags and the press/release state. This feeds
    /// [`AppAction::PointerRelease`].
    pub(crate) fn classify_pointer_release(&self, px: f32, py: f32) -> ReleaseAction {
        // The context menu takes priority over the folder panel on release too.
        if self.context_menu.is_active() {
            return ReleaseAction::ContextMenu;
        }
        if self.folders.is_active() && self.drag_item.is_none() && !self.pressed_on_control {
            return ReleaseAction::Folder;
        }
        let settings_pressed = if self.settings_open {
            self.pressed_on_settings
        } else {
            None
        };
        let settings_released = if self.settings_open {
            self.settings_hit_target(px, py)
        } else {
            super::state::SettingsPressTarget::Outside
        };
        let on_capsule = if self.pressed_on_control {
            self.bottom_control_capsule_hit(px, py)
        } else {
            false
        };
        let editing_with_drag = self.editing && self.drag_item.is_some();
        let has_pending_press = self.pending_press.is_some();
        let is_outside_glass_click = self
            .pending_press
            .as_ref()
            .map(|p| p.is_outside_glass_click(px, py))
            .unwrap_or(false);
        let has_launch_id = self
            .pending_press
            .as_ref()
            .and_then(|p| p.activated_item(px, py))
            .is_some();
        let scroller_dragging = self
            .scroller
            .as_ref()
            .map(|s| s.phase == Phase::Dragging)
            .unwrap_or(false);
        pointer_release_action(
            self.settings_open,
            settings_pressed,
            settings_released,
            self.pressed_on_control,
            on_capsule,
            editing_with_drag,
            has_pending_press,
            is_outside_glass_click,
            has_launch_id,
            scroller_dragging,
        )
    }
}
