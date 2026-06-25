#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Launchpad (Windows) — MVP entry point.
//!
//! Wires winit's event loop to the wgpu renderer and the scroll physics.
//! Operation:
//!   - Left-drag horizontally → page swipe with rubber-band + spring snap.
//!   - Click an app icon → launch its Start Menu shortcut.
//!   - Esc → quit.
//!
//! The window keeps redrawing only while the scroller is animating; when it
//! settles, we stop requesting frames to keep CPU/GPU idle.

mod grid;
mod icon_pipeline;
mod icons;
mod launch;
mod liquid_glass;
mod renderer;
mod scroll;
mod text;

use std::time::Instant;

use icons::LoadedIcons;
use renderer::{DrawArgs, Renderer};
use scroll::{Phase, Scroller};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

const CLICK_SLOP_PHYS: f32 = 8.0;

#[derive(Debug, Clone, Copy)]
pub(crate) enum UserEvent {
    BackdropFrameArrived,
}

/// Owns the renderer (which owns the window) plus the scroll state.
struct App {
    event_proxy: EventLoopProxy<UserEvent>,
    renderer: Option<Renderer>,
    scroller: Option<Scroller>,
    text: Option<text::TextRenderer>,
    layout: grid::GridLayout,
    /// Loaded Start Menu apps + packed icon atlas. `None` until the first
    /// `resumed` (lazy-loaded so the window appears before icon extraction).
    loaded_icons: Option<LoadedIcons>,
    /// Logical→physical scale factor for text layout only. Winit cursor events
    /// already arrive in physical pixels.
    scale_factor: f32,
    /// Last known pointer x in physical px.
    pointer_phys_x: f32,
    /// Last known pointer y in physical px.
    pointer_phys_y: f32,
    /// Pointer x at drag start (physical px).
    drag_start_x: f32,
    /// Pointer y at drag start (physical px).
    drag_start_y: f32,
}

impl App {
    fn new(event_proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            event_proxy,
            renderer: None,
            scroller: None,
            text: None,
            layout: grid::GridLayout::default(),
            loaded_icons: None,
            scale_factor: 1.0,
            pointer_phys_x: 0.0,
            pointer_phys_y: 0.0,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
        }
    }

    fn viewport_phys(&self) -> (u32, u32) {
        self.renderer
            .as_ref()
            .map(|r| {
                let s = r.window.inner_size();
                (s.width, s.height)
            })
            .unwrap_or((1280, 800))
    }

    /// Recompute layout/bounds for the current window size and push the new
    /// tile positions + labels to the GPU.
    fn relayout(&mut self) {
        let (w, _h) = self.viewport_phys();
        self.layout = grid::GridLayout::default().centered(w as f32);
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.set_bounds(bounds);
        }

        // The app list (empty until icons finish loading) drives tile colors,
        // labels, and icon instances.
        let apps: &[icons::AppEntry] = self
            .loaded_icons
            .as_ref()
            .map(|li| li.apps.as_slice())
            .unwrap_or(&[]);

        // Build glyph quads from labels via the text renderer, then upload.
        let scale = self.scale_factor;
        let dirty = if let Some(t) = self.text.as_mut() {
            let labels = self.layout.build_labels(w as f32, apps);
            let quads = t.layout_labels(&labels, scale);
            let dirty = t.atlas_dirty;
            // Upload quads + atlas to the renderer.
            if let Some(r) = self.renderer.as_mut() {
                r.set_text_instances(&quads);
                if dirty {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            dirty
        } else {
            false
        };
        if dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        // Rebuild the GPU tile instance buffer so tiles re-center on the new size.
        if let Some(r) = self.renderer.as_mut() {
            r.rebuild_instances(&self.layout, apps);

            // Build per-icon instances (one per tile that has an icon UV) and
            // upload them. The atlas itself is uploaded once, when it first
            // becomes available (see load_icons_if_needed).
            let icon_instances = self.layout.build_icon_instances(w as f32, apps);
            r.set_icon_instances(&icon_instances);
        }
    }

    /// Load Start Menu icons synchronously on first need, then upload the
    /// atlas. Idempotent: a no-op once already loaded. Called from `resumed`.
    fn load_icons_if_needed(&mut self) {
        if self.loaded_icons.is_some() {
            return;
        }
        let loaded = icons::load_all_icons();
        eprintln!(
            "loaded {} apps, {} with icons (atlas {}x{})",
            loaded.apps.len(),
            loaded.apps.iter().filter(|a| a.uv.is_some()).count(),
            loaded.atlas.width,
            loaded.atlas.height,
        );
        if let Some(r) = self.renderer.as_mut() {
            r.upload_icon_atlas(&loaded.atlas.rgba, loaded.atlas.width, loaded.atlas.height);
        }
        self.loaded_icons = Some(loaded);
    }

    fn handle_drag_start(&mut self, x_phys: f32, y_phys: f32) {
        self.drag_start_x = x_phys;
        self.drag_start_y = y_phys;
        if let Some(s) = self.scroller.as_mut() {
            s.drag_start(x_phys);
        }
        self.request_redraw();
    }

    fn handle_drag_move(&mut self, x_phys: f32) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_move(x_phys);
        }
        self.request_redraw();
    }

    fn handle_drag_end(&mut self) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_end();
        }
        self.request_redraw();
    }

    fn handle_pointer_release(&mut self) -> bool {
        let x = self.pointer_phys_x;
        let y = self.pointer_phys_y;
        let dx = x - self.drag_start_x;
        let dy = y - self.drag_start_y;
        let is_click = dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS;

        let launched = is_click && self.launch_app_at(x, y);
        self.handle_drag_end();
        launched
    }

    fn launch_app_at(&self, x_phys: f32, y_phys: f32) -> bool {
        let Some(loaded) = self.loaded_icons.as_ref() else {
            return false;
        };
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let Some(app_index) =
            self.layout
                .hit_test_app(w as f32, x_phys, y_phys, scroll_x, loaded.apps.len())
        else {
            return false;
        };
        let Some(app) = loaded.apps.get(app_index) else {
            return false;
        };

        match launch::open_shortcut(&app.link_path) {
            Ok(()) => {
                eprintln!("launched {}", app.name);
                true
            }
            Err(err) => {
                eprintln!(
                    "failed to launch {} ({}): {}",
                    app.name,
                    app.link_path.display(),
                    err
                );
                false
            }
        }
    }

    fn request_redraw(&self) {
        if let Some(r) = self.renderer.as_ref() {
            r.window.request_redraw();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::BackdropFrameArrived => self.request_redraw(),
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
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
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(640.0, 480.0));

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
        self.layout = grid::GridLayout::default().centered(w as f32);

        let renderer = pollster::block_on(Renderer::new(
            window,
            &self.layout,
            self.event_proxy.clone(),
        ))
        .expect("init renderer");
        let bounds = self.layout.bounds(w as f32);
        let scroller = Scroller::new(bounds);
        let text = text::TextRenderer::new();

        self.renderer = Some(renderer);
        self.scroller = Some(scroller);
        self.text = Some(text);
        // Load Start Menu icons + atlas before the first layout so the initial
        // frame already shows real icons. This is synchronous (one-time cost
        // at startup); scrolling later touches none of it.
        self.load_icons_if_needed();
        // Lay out the initial labels and upload to the GPU.
        self.relayout();
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let winit::keyboard::PhysicalKey::Code(key_code) = event.physical_key else {
                    return;
                };

                if key_code == winit::keyboard::KeyCode::Escape {
                    event_loop.exit();
                    return;
                }

                // M toggles the OS window frame on/off for easier debugging
                // (grab edges to resize, title bar to move) without rebuilding.
                if key_code == winit::keyboard::KeyCode::KeyM {
                    if let Some(r) = self.renderer.as_mut() {
                        r.toggle_decorations();
                        self.request_redraw();
                    }
                    return;
                }

                if let Some(r) = self.renderer.as_mut() {
                    if r.handle_liquid_glass_key(key_code) {
                        self.request_redraw();
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
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_phys_x = position.x as f32;
                self.pointer_phys_y = position.y as f32;
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
                        self.handle_drag_start(self.pointer_phys_x, self.pointer_phys_y);
                    }
                    ElementState::Released => {
                        if self.handle_pointer_release() {
                            if let Some(r) = self.renderer.as_ref() {
                                r.window.set_visible(false);
                            }
                            event_loop.exit();
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let vp = self.viewport_phys();
                let animating;
                if let (Some(r), Some(s)) = (self.renderer.as_mut(), self.scroller.as_mut()) {
                    let dragging = s.phase == Phase::Dragging;
                    s.tick(now);
                    r.render(&DrawArgs {
                        scroll_x: s.position,
                        viewport: vp,
                        defer_backdrop_capture: dragging,
                    });
                    animating = s.is_animating();
                } else {
                    return;
                }
                if animating {
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Keep the loop pumping while animating; otherwise winit blocks until
        // the next input or WGC FrameArrived user event.
        let animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        if animating {
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new(event_loop.create_proxy());
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}
