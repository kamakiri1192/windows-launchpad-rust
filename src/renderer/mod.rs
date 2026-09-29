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
//! - [`prepare`]: renderer-neutral scene preparation and dirty tracking.
//! - [`badges`]: edit-badge glass + foreground geometry.
//! - [`frame`]: per-frame draw-pass orchestration and QA capture.
//!
//! Note: written against the wgpu 29 API.

mod badges;
pub(crate) mod controls;
mod counters;
mod focus_blur;
mod fps;
mod frame;
pub(crate) mod gpu_profile;
pub(crate) mod icon_atlas;
pub(crate) mod icon_pipeline;
mod icons;
mod init;
mod prepare;
#[cfg(target_os = "windows")]
mod present_stats;
mod resources;
mod text;
pub(crate) mod text_engine;
pub(crate) mod tiles;

use std::sync::Arc;

use wgpu::{
    Buffer, Device, Queue, RenderPipeline, Surface, SurfaceConfiguration, Texture, TextureFormat,
};

use crate::layout::grid::GridLayout;
use crate::liquid_glass::LiquidGlassRenderer;
use crate::renderer::controls::{ControlInstance, KIND_ROUND_RECT};
use crate::renderer::text_engine::GlyphQuad;

use counters::BufferCounters;
use resources::InstanceBuffer;

pub(crate) use badges::EditBadgeSource;

pub struct Renderer {
    /// Owned window. Kept here so the surface (which borrows it) is valid.
    pub window: winit::window::Window,
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub surface: Option<Surface<'static>>,
    /// Surface-free render target for deterministic QA on hosted macOS
    /// runners, where the Actions service has no WindowServer surface.
    qa_offscreen: Option<Texture>,
    pub config: SurfaceConfiguration,
    pipeline: RenderPipeline,
    /// Current decorations state (borderless by default, toggle with M).
    decorated: bool,
    /// Static per-tile instance data (capacity-managed; see `resources`).
    instance_buffer: InstanceBuffer<crate::renderer::tiles::TileInstance>,
    /// Whether the trailing top-level tile belongs to the lifted drag lane.
    /// A modal folder-child drag also sets the global drag uniform, but must
    /// not withhold an unrelated top-level tile from the normal pass.
    top_level_dragged_tile_instance: bool,
    modal_tile_instance_buffer: InstanceBuffer<crate::renderer::tiles::TileInstance>,
    modal_dragged_tile_instance: bool,
    context_menu_tile_instance_buffer: InstanceBuffer<crate::renderer::tiles::TileInstance>,
    /// Per-frame uniform data (viewport + scroll).
    uniform_buffer: Buffer,
    uniform_bind_group: wgpu::BindGroup,
    /// Current sRGB surface format (saved for future MSAA / gamma work).
    #[allow(dead_code)]
    surface_format: TextureFormat,
    liquid_glass: LiquidGlassRenderer,
    focus_blur: focus_blur::FocusBlurRenderer,
    gpu_profiler: gpu_profile::GpuProfilerState,
    /// Bumped whenever `prepare` observes changes in the model lanes that are
    /// composited below each completed-scene checkpoint, so the checkpoint
    /// caches can detect stale inputs without being invalidated by the
    /// checkpoint's own animated lanes.
    below_settings_epoch: u64,
    below_menu_epoch: u64,
    /// Last scene-content epoch for which the completed-scene chain
    /// (context-menu/settings copy + flatten + blur) ran, keyed by lane.
    completed_scene_cache: Option<(u64, CompletedSceneLane)>,
    /// Last scene-texture key for which the focus veil pyramid ran.
    focus_blur_cache: Option<u64>,
    /// Presentation-rate estimator feeding the debug HUD. On Windows it
    /// prefers DXGI frame statistics; elsewhere it tracks `present()` cadence.
    pub(crate) fps_tracker: fps::FpsTracker,
    /// Cached reading of [`fps_tracker`](Self::fps_tracker), updated at the
    /// end of each successful `render()` so callers (the app layer building
    /// HUD glyphs) read a stable value without borrowing the tracker.
    pub last_fps: u32,

    // -- Text rendering -------------------------------------------------
    text_pipeline: RenderPipeline,
    text_instance_buffer: InstanceBuffer<GlyphQuad>,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    /// Copy of the bind group layout (for texture/sampler + uniform).
    /// Reused to rebuild the atlas bind groups when either atlas grows.
    text_bgl: wgpu::BindGroupLayout,

    // -- Icon rendering -------------------------------------------------
    icon_pipeline: RenderPipeline,
    icon_instance_buffer: InstanceBuffer<crate::renderer::icon_pipeline::IconInstance>,
    modal_icon_instance_buffer: InstanceBuffer<crate::renderer::icon_pipeline::IconInstance>,
    modal_dragged_icon_instance: bool,
    context_menu_icon_instance_buffer: InstanceBuffer<crate::renderer::icon_pipeline::IconInstance>,
    /// Number of trailing top-level icon instances belonging to the dragged
    /// visual unit. Apps use one; a closed folder uses all preview miniatures.
    dragged_icon_instance_count: u32,
    icon_atlas_texture: wgpu::Texture,
    icon_atlas_bind_group: wgpu::BindGroup,

    // -- Frame clip for tiles ------------------------------------------
    // Fixed page-frame geometry in physical px, fed to the tile/icon/text
    // shaders so they clip to the frame's rounded rect. `(cx, cy, hw, hh, r)`.
    frame_clip: (f32, f32, f32, f32, f32),
    /// Rectangular GPU scissor for generic modal content. It follows the
    /// highest-z modal glass surface so paged folder children cannot leak
    /// outside their panel while sliding.
    modal_clip_rect: Option<crate::ui_model::geometry::Rect>,
    modal_clip_radius: f32,
    context_menu_clip_rect: Option<crate::ui_model::geometry::Rect>,
    context_menu_clip_radius: f32,

    // -- Bottom control overlays --------------------------------------
    // The control's glass capsule is drawn by the Liquid Glass pass (it's a
    // shape in the geometry buffer). These two pipelines draw the foreground
    // ink on top: procedural shapes (magnifier, dots, caret, close) and the
    // cosmic-text glyphs for the label / query / placeholder.
    control_pipeline: RenderPipeline,
    control_uniform_buffer: Buffer,
    control_bind_group: wgpu::BindGroup,
    /// Control bind group layout, kept so the control bind groups can be
    /// rebuilt when the glyph atlas grows (they sample the atlas).
    control_bgl: wgpu::BindGroupLayout,
    /// Brand-logo SDF atlas (binding 3 of the control bind group). A 2D array
    /// texture whose layer 0 is the ChatGPT logo and layer 1 is the Steam logo,
    /// each a single-channel signed distance field. Kept on the struct so it
    /// outlives the bind group that references it.
    logo_sdf_atlas_texture: wgpu::Texture,
    /// View into [`logo_sdf_atlas_texture`]. Cached so the control bind group
    /// can be rebuilt (on glyph-atlas growth) without re-deriving it each time.
    logo_sdf_atlas_view: wgpu::TextureView,
    /// Linear sampler for [`logo_sdf_atlas_texture`]. Cached for the same reason
    /// as the view.
    logo_sdf_atlas_sampler: wgpu::Sampler,
    control_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    backdrop_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    /// Corner gear ink instances (settings entry). Drawn in the control
    /// overlay pass alongside the bottom-control ink.
    gear_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    badge_sources: Vec<EditBadgeSource>,
    badge_shape_scratch: Vec<crate::liquid_glass::geometry::GlassShape>,
    badge_mark_scratch: Vec<crate::renderer::controls::ControlInstance>,
    badge_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    modal_badge_sources: Vec<EditBadgeSource>,
    modal_badge_shape_scratch: Vec<crate::liquid_glass::geometry::GlassShape>,
    modal_badge_mark_scratch: Vec<crate::renderer::controls::ControlInstance>,
    modal_badge_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    control_text_pipeline: RenderPipeline,
    control_text_bind_group: wgpu::BindGroup,
    control_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    /// Debug HUD glyphs (e.g. "FPS: 60"), drawn in the final overlay pass
    /// on top of all modal content. Populated by the app layer from
    /// [`Self::last_fps`]; reuses the control-text pipeline + atlas.
    debug_overlay_backgrounds: Vec<DebugOverlayBackground>,
    overlay_background_instance_buffer: InstanceBuffer<ControlInstance>,
    overlay_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    /// Settings overlay ink (close ×) + title text instances, drawn in a final
    /// overlay pass on top of the panel glass. They reuse the control pipelines.
    settings_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    settings_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    modal_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    modal_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    context_menu_instance_buffer: InstanceBuffer<crate::renderer::controls::ControlInstance>,
    context_menu_text_instance_buffer: InstanceBuffer<GlyphQuad>,
    /// Last prepared neutral scene, updated lane-by-lane only when changed.
    prepared_model: crate::ui_model::render_model::RenderModel,
    /// Debug-only allocation/upload counters. Zero-sized in release builds.
    counters: BufferCounters,
    /// When set, the next rendered frame is also copied to a host-readable
    /// buffer and saved as a PNG at this path. Driven by the
    /// `LAUNCHPAD_QA_SHOT_FILE` trigger (see `docs/EDIT_MODE_VISUAL_QA.md`) so
    /// CI / sandboxes without foreground access can capture rendered frames.
    /// Cleared after one frame.
    pub qa_shot: Option<std::path::PathBuf>,
}

/// Which surface owns the current completed-scene blur cache entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompletedSceneLane {
    Settings,
    ContextMenu,
}

/// A compact dark backing plate for one line of the debug HUD.
///
/// The backing plate is deliberately a procedural rounded rectangle rather
/// than a text shadow. It adds one small control instance per visible line,
/// keeps the glyph shader at one atlas sample, and avoids multiplying the
/// number of glyph fragments for a diagnostic that is often used while
/// measuring frame pacing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DebugOverlayBackground {
    pub(crate) center: [f32; 2],
    pub(crate) half_size: [f32; 2],
    pub(crate) radius: f32,
    pub(crate) color: [f32; 4],
    pub(crate) opacity: f32,
}

impl Renderer {
    /// Upload the procedural backing plates for the final debug HUD pass.
    pub(crate) fn set_debug_overlay_backgrounds(&mut self, backgrounds: &[DebugOverlayBackground]) {
        // The FPS number changes every frame, but its measured width usually
        // does not. Avoid rewriting the tiny GPU buffer unless line geometry
        // actually changes (DPI, enabled lines, or a digit-width transition).
        if self.debug_overlay_backgrounds.as_slice() == backgrounds {
            return;
        }
        self.debug_overlay_backgrounds.clear();
        self.debug_overlay_backgrounds
            .extend_from_slice(backgrounds);

        let instances: Vec<_> = backgrounds
            .iter()
            .map(|background| ControlInstance {
                center: background.center,
                params: [
                    background.half_size[1],
                    background.opacity,
                    background.half_size[0],
                    background.radius,
                ],
                color: background.color,
                kind: [KIND_ROUND_RECT, 0.0, 0.0, 0.0],
                clip_rect: [0.0; 4],
                clip_radius: [0.0; 4],
            })
            .collect();
        let outcome =
            self.overlay_background_instance_buffer
                .set(&self.device, &self.queue, &instances);
        if outcome.allocated {
            self.counters.record_growth(counters::Category::Control);
        }
        if outcome.wrote {
            self.counters.record_write(counters::Category::Control);
        }
    }
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
    /// Open-transition values: `(gather_progress, opacity, start_scale,
    /// max_stagger)`.
    pub presentation: [f32; 4],
    /// Open-transition motion values: `(start_offset_fraction, _, _, _)`.
    pub presentation_motion: [f32; 4],
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
        layout.scaled(crate::layout::grid::FRAME_CORNER_RADIUS),
    )
}
