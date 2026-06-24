//! Launchpad (Windows) — MVP entry point.
//!
//! Wires winit's event loop to the wgpu renderer and the scroll physics.
//! Operation:
//!   - Left-drag horizontally → page swipe with rubber-band + spring snap.
//!   - Esc → quit.
//!
//! The window keeps redrawing only while the scroller is animating; when it
//! settles, we stop requesting frames to keep CPU/GPU idle.

mod grid;
mod renderer;
mod scroll;
mod text;

use std::time::Instant;

use renderer::{DrawArgs, Renderer};
use scroll::{Phase, Scroller};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Owns the renderer (which owns the window) plus the scroll state.
struct App {
    renderer: Option<Renderer>,
    scroller: Option<Scroller>,
    text: Option<text::TextRenderer>,
    layout: grid::GridLayout,
    /// Logical→physical scale factor, for converting pointer deltas.
    scale_factor: f32,
    /// Last known pointer x in logical px.
    pointer_logical_x: f32,
    /// Pointer x at drag start (physical px).
    drag_start_x: f32,
}

impl App {
    fn new() -> Self {
        Self {
            renderer: None,
            scroller: None,
            text: None,
            layout: grid::GridLayout::default(),
            scale_factor: 1.0,
            pointer_logical_x: 0.0,
            drag_start_x: 0.0,
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

        // Build glyph quads from labels via the text renderer, then upload.
        let scale = self.scale_factor;
        let dirty = if let Some(t) = self.text.as_mut() {
            let labels = self.layout.build_labels(w as f32);
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
            r.rebuild_instances(&self.layout);
        }
    }

    fn handle_drag_start(&mut self, x_logical: f32) {
        let x = x_logical * self.scale_factor;
        self.drag_start_x = x;
        if let Some(s) = self.scroller.as_mut() {
            s.drag_start(x);
        }
        self.request_redraw();
    }

    fn handle_drag_move(&mut self, x_logical: f32) {
        let x = x_logical * self.scale_factor;
        if let Some(s) = self.scroller.as_mut() {
            s.drag_move(x);
        }
        self.request_redraw();
    }

    fn handle_drag_end(&mut self) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_end();
        }
        self.request_redraw();
    }

    fn request_redraw(&self) {
        if let Some(r) = self.renderer.as_ref() {
            r.window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Launchpad")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(640.0, 480.0));

        let window = event_loop.create_window(attrs).expect("create window");
        self.scale_factor = window.scale_factor() as f32;
        let (w, _h) = (window.inner_size().width, window.inner_size().height);
        self.layout = grid::GridLayout::default().centered(w as f32);

        let renderer =
            pollster::block_on(Renderer::new(window, &self.layout)).expect("init renderer");
        let bounds = self.layout.bounds(w as f32);
        let scroller = Scroller::new(bounds);
        let text = text::TextRenderer::new();

        self.renderer = Some(renderer);
        self.scroller = Some(scroller);
        self.text = Some(text);
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
                if event.state == ElementState::Pressed
                    && event.physical_key == winit::keyboard::KeyCode::Escape
                {
                    event_loop.exit();
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
                self.pointer_logical_x = position.x as f32;
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
                        self.handle_drag_start(self.pointer_logical_x);
                    }
                    ElementState::Released => self.handle_drag_end(),
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let vp = self.viewport_phys();
                let animating;
                if let (Some(r), Some(s)) = (self.renderer.as_ref(), self.scroller.as_mut()) {
                    s.tick(now);
                    r.render(&DrawArgs {
                        scroll_x: s.position,
                        viewport: vp,
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Keep the loop pumping while animating; otherwise winit blocks until
        // the next event (good for idle CPU).
        let animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        if animating {
            self.request_redraw();
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new();
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}
