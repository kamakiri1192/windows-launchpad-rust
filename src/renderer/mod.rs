//! wgpu renderer facade for the Launchpad MVP.
//!
//! Owns the window, device/queue, surface, render pipelines, and GPU buffers.
//! The instance buffer is written **once** (tiles are static); only the
//! ~16-byte uniform (viewport + scroll offset) is updated per frame, so
//! scrolling costs essentially nothing on the CPU/GPU bus.
//!
//! The `Window` is moved into this struct so that the `Surface` can borrow it
//! for `'static` — both live and die together.
//!
//! The facade is split into focused submodules:
//! - [`init`]: device/surface/pipeline creation and lifecycle.
//! - [`tiles`]: static tile instance buffer.
//! - [`icons`]: icon atlas + icon instance buffer.
//! - [`text`]: glyph atlas + text instance buffer.
//! - [`controls`]: procedural overlay instance buffers (control/gear/settings).
//! - [`glass`]: Liquid Glass shape submission.
//! - [`badges`]: edit-badge glass + foreground geometry.
//! - [`frame`]: per-frame draw-pass orchestration and QA capture.
//!
//! Note: written against the wgpu 29 API.

mod badges;
mod controls;
mod counters;
mod frame;
mod glass;
mod icons;
mod init;
mod resources;
mod text;
mod tiles;

use std::sync::Arc;

use wgpu::{Buffer, Device, Queue, RenderPipeline, Surface, SurfaceConfiguration, TextureFormat};

use crate::grid::GridLayout;
use crate::liquid_glass::LiquidGlassRenderer;
use crate::text::GlyphQuad;

use counters::BufferCounters;
use resources::InstanceBuffer;

pub(crate) use badges::EditBadgeSource;

pub struct Renderer {
    /// Owned window. Kept here so the surface (which borrows it) is valid.
    pub window: winit::window::Window,
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    pipeline: RenderPipeline,
    /// Current decorations state (borderless by default, toggle with M).
    decorated: bool,
    /// Static per-tile instance data (capacity-managed; see `resources`).
    instance_buffer: InstanceBuffer<crate::grid::TileInstance>,
    /// Per-frame uniform data (viewport + scroll).
    uniform_buffer: Buffer,
    uniform_bind_group: wgpu::BindGroup,
    /// Current sRGB surface format (saved for future MSAA / gamma work).
    #[allow(dead_code)]
    surface_format: TextureFormat,
    liquid_glass: LiquidGlassRenderer,

    // -- Text rendering -------------------------------------------------
    text_pipeline: RenderPipeline,
    text_instance_buffer: InstanceBuffer<GlyphQuad>,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    /// Copy of the bind group layout (for texture/sampler + uniform).
    #[allow(dead_code)]
    text_bgl: wgpu::BindGroupLayout,

    // -- Icon rendering -------------------------------------------------
    icon_pipeline: RenderPipeline,
    icon_instance_buffer: InstanceBuffer<crate::icon_pipeline::IconInstance>,
    dragged_icon_instance: bool,
    icon_atlas_texture: wgpu::Texture,
    icon_atlas_bind_group: wgpu::BindGroup,

    // -- Frame clip for tiles ------------------------------------------
    // Fixed page-frame geometry in physical px, fed to the tile/icon/text
    // shaders so they clip to the frame's rounded rect. `(cx, cy, hw, hh, r)`.
    frame_clip: (f32, f32, f32, f32, f32),

    // -- Bottom control overlays --------------------------------------
    // The control's glass capsule is drawn by the Liquid Glass pass (it's a
    // shape in the geometry buffer). These two pipelines draw the foreground
    // ink on top: procedural shapes (magnifier, dots, caret, close) and the
    // cosmic-text glyphs for the label / query / placeholder.
    control_pipeline: RenderPipeline,
    control_uniform_buffer: Buffer,
    control_bind_group: wgpu::BindGroup,
    control_instance_buffer: InstanceBuffer<crate::bottom_control::ControlInstance>,
    /// Corner gear ink instances (settings entry). Drawn in the control
    /// overlay pass alongside the bottom-control ink.
    gear_instance_buffer: InstanceBuffer<crate::bottom_control::ControlInstance>,
    badge_sources: Vec<EditBadgeSource>,
    badge_instance_buffer: InstanceBuffer<crate::bottom_control::ControlInstance>,
    control_text_pipeline: RenderPipeline,
    control_text_bind_group: wgpu::BindGroup,
    control_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    /// Settings overlay ink (close ×) + title text instances, drawn in a final
    /// overlay pass on top of the panel glass. They reuse the control pipelines.
    settings_instance_buffer: InstanceBuffer<crate::bottom_control::ControlInstance>,
    settings_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    /// Debug-only allocation/upload counters. Zero-sized in release builds.
    counters: BufferCounters,
    /// When set, the next rendered frame is also copied to a host-readable
    /// buffer and saved as a PNG at this path. Driven by the
    /// `LAUNCHPAD_QA_SHOT_FILE` trigger (see `docs/EDIT_MODE_VISUAL_QA.md`) so
    /// CI / sandboxes without foreground access can capture rendered frames.
    /// Cleared after one frame.
    pub qa_shot: Option<std::path::PathBuf>,
}

pub struct DrawArgs {
    pub scroll_x: f32,
    pub viewport: (u32, u32),
    pub defer_backdrop_capture: bool,
    /// Global animation clock in seconds, fed to the shaders for the edit-mode
    /// wiggle. Caller accumulates this from the redraw cadence.
    pub time: f32,
    /// 1.0 while an edit-mode drag is in flight, else 0.0.
    pub drag_active: f32,
    /// Pointer position (screen px) the dragged icon follows while dragging.
    pub drag_pos: (f32, f32),
}

/// Frame clip geometry for the tile/icon/text shaders: `(cx, cy, hw, hh, r)`
/// — center, half-size, and corner radius of the fixed page frame, in physical
/// px. Single source is `GridLayout::frame_panel_rect`.
pub(super) fn frame_clip(layout: &GridLayout, viewport_w: u32) -> (f32, f32, f32, f32, f32) {
    let (cx, cy, w, h) = layout.frame_panel_rect(viewport_w.max(1) as f32);
    (
        cx,
        cy,
        w * 0.5,
        h * 0.5,
        layout.scaled(crate::grid::FRAME_CORNER_RADIUS),
    )
}
