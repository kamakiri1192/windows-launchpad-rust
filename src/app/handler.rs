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
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

use crate::debug_log;
use crate::grid;
use crate::liquid_glass;
use crate::renderer::text_engine as text;
use crate::renderer::Renderer;
use crate::scroll::{Phase, Scroller};
use crate::startup_timer::prefix;

use super::action::{
    folder_keyboard_action, keyboard_action, pointer_press_action, pointer_release_action,
    AppAction, PressAction, ReleaseAction,
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
        };
        self.handle_action(action);
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
                let key_action = if self.folders.is_active() && !self.settings_open {
                    folder_keyboard_action(
                        self.folders.rename.is_some(),
                        self.editing,
                        self.folders
                            .rename
                            .as_ref()
                            .is_none_or(|editor| editor.preedit.is_empty()),
                        key_code,
                        event.text.as_deref(),
                    )
                } else {
                    keyboard_action(
                        self.settings_open,
                        self.editing,
                        self.control.wants_keyboard(),
                        self.control.preedit.is_empty(),
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
            WindowEvent::Moved(_) => AppAction::Moved,
            WindowEvent::CursorLeft { .. } => AppAction::CursorLeft,
            WindowEvent::CursorMoved { position, .. } => AppAction::PointerMoved {
                x: position.x as f32,
                y: position.y as f32,
            },
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                let px = self.pointer_phys_x;
                let py = self.pointer_phys_y;
                match state {
                    ElementState::Pressed => {
                        let action = self.classify_pointer_press(px, py);
                        AppAction::PointerPress(action)
                    }
                    ElementState::Released => {
                        let action = self.classify_pointer_release(px, py);
                        AppAction::PointerRelease(action)
                    }
                }
            }
            WindowEvent::RedrawRequested => AppAction::RedrawRequested,
            WindowEvent::Focused(focused) => AppAction::Focused(focused),
            _ => return,
        };
        self.handle_action(action);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Real quit path: the tray "Quit" command set the flag; now that the
        // current event is fully handled we can terminate the loop.
        if self.should_quit {
            event_loop.exit();
            return;
        }

        // Dispatch a tick action (long-press check + animation-gated redraw).
        self.handle_action(AppAction::Tick {
            now: Instant::now(),
        });
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

impl App {
    /// Classify a left-button press into a [`PressAction`] using the current
    /// shell flags and the pointer position. This feeds
    /// [`AppAction::PointerPress`].
    fn classify_pointer_press(&self, px: f32, py: f32) -> PressAction {
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
    fn classify_pointer_release(&self, px: f32, py: f32) -> ReleaseAction {
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
