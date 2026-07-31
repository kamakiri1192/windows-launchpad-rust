//! Text rendering for the Launchpad MVP.
//!
//! Uses `cosmic-text` to shape/layout each label (Japanese-capable) and
//! `SwashCache` to rasterize glyphs into a CPU-side texture atlas. The atlas
//! is re-uploaded to the GPU whenever it becomes dirty; the renderer
//! instance-draws one quad per glyph, sampling the atlas.
//!
//! When the atlas fills up it **grows** (doubling, up to [`ATLAS_MAX_SIZE`]):
//! existing glyphs keep their exact pixel positions so nothing is evicted or
//! re-rasterized. Growth only changes the UV denominators, so the app layer
//! rebuilds every glyph lane once per growth (see
//! [`TextRenderer::take_atlas_grew`]) — the same pattern the icon atlas
//! already uses.
//!
//! The layout works in **two phases** to keep Rust's borrow checker happy
//! (both `Buffer` layout and `SwashCache` need `&mut FontSystem`):
//!   1. *Layout phase*: run cosmic-text per label, collect every glyph as a
//!      `(PhysicalGlyph, on-screen position)` pair.
//!   2. *Raster phase*: for each unique glyph, ensure it's in the atlas
//!      (rasterizing on cache miss) and emit a `GlyphQuad`.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, PhysicalGlyph, Shaping, SwashCache, Weight,
    Wrap,
};

/// A drawable glyph quad, matching the WGSL instance attributes for the text
/// pipeline. 80 bytes for clean GPU alignment (5 vec4s).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlyphQuad {
    /// Top-left corner in content pixels.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// UV rectangle into the atlas, in 0..1.
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    /// Non-premultiplied RGBA tint applied in the fragment shader.
    pub color: [f32; 4],
    /// Clip rectangle in physical px: (min_x, min_y, width, height).
    /// Sentinel: clip_rect.z <= 0.0 means "no clip".
    pub clip_rect: [f32; 4],
    /// Clip corner radius in physical px (0 = sharp corners).
    /// Packed as vec4 with padding: (radius, 0, 0, 0).
    pub clip_radius: [f32; 4],
}

impl GlyphQuad {
    pub const ATTRIBS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x4,
        1 => Float32x4,
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4
    ];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GlyphQuad>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &GlyphQuad::ATTRIBS,
    };

    fn with_offset_and_color(mut self, dx: f32, dy: f32, color: [f32; 4]) -> Self {
        self.x += dx;
        self.y += dy;
        self.color = color;
        self
    }
}

/// One entry in the atlas: where the glyph bitmap lives (in pixels).
#[derive(Debug, Clone, Copy)]
struct AtlasEntry {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// Offset from the pen position (physical.x/y) to the glyph bitmap's
    /// top-left, derived from swash's `placement.left`/`placement.top`.
    off_x: i32,
    off_y: i32,
}

/// A label to lay out: the text plus the on-screen anchor.
pub struct Label {
    pub text: String,
    /// Top-left X of the label box (content px).
    pub x: f32,
    /// Top-left Y of the label box (content px).
    pub y: f32,
    /// Max width before wrapping (content px).
    pub max_width: f32,
    /// Non-premultiplied RGBA tint. Folder labels use this to preserve the
    /// panel open/close fade while sharing the normal launcher label layout.
    pub color: [f32; 4],
}

/// Intermediate record from the layout phase.
struct PlacedGlyph {
    physical: PhysicalGlyph,
    /// On-screen glyph origin before applying the raster image placement.
    x: f32,
    y: f32,
    color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LabelLayoutKey {
    text: String,
    max_width_bits: u32,
    scale_factor_bits: u32,
}

#[derive(Debug, Clone)]
struct CachedLabelGlyph {
    physical: PhysicalGlyph,
    /// Position relative to the label box's top-left corner.
    x: f32,
    y: f32,
}

/// Parameters for [`TextRenderer::layout_centered_line`]: a single centered
/// line of text with an explicit color. Bundled into a struct so the method
/// stays under clippy's argument-count limit.
pub struct CenteredLineSpec<'a> {
    pub text: &'a str,
    pub font_size: f32,
    pub line_height: f32,
    pub family: &'a str,
    pub color: [f32; 4],
    /// On-screen center of the line, in physical px.
    pub center: (f32, f32),
    pub scale_factor: f32,
}

pub struct TextRenderer {
    font_system: FontSystem,
    swash: SwashCache,
    /// Atlas RGBA buffer (CPU side), row-major, `atlas_w * atlas_h * 4` bytes.
    atlas: Vec<u8>,
    /// Current atlas dimensions in pixels. Start at [`ATLAS_INITIAL_SIZE`]
    /// and double (up to [`ATLAS_MAX_SIZE`]) whenever the row packer runs
    /// out of space; existing glyphs keep their pixel positions.
    atlas_w: u32,
    atlas_h: u32,
    /// Cache key → atlas placement.
    cache: HashMap<cosmic_text::CacheKey, AtlasEntry>,
    /// Next free cell cursor for the row packer.
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    /// True if the atlas changed since the last GPU upload.
    pub atlas_dirty: bool,
    /// True if the atlas grew since the last [`Self::take_atlas_grew`] call.
    /// Growth changes the UV denominators of every previously built glyph
    /// quad, so the app must rebuild all glyph lanes once.
    atlas_grew: bool,
    /// Shaping is independent of a label's on-screen position. Folder paging
    /// changes only that position, so retain relative glyph layouts instead
    /// of asking cosmic-text to shape every visible name on every frame.
    label_layout_cache: HashMap<LabelLayoutKey, Vec<CachedLabelGlyph>>,
}

/// Initial (square) atlas size in pixels. The GPU texture is created at this
/// size and reallocated when the atlas grows.
pub const ATLAS_INITIAL_SIZE: u32 = 1024;
/// Maximum (square) atlas size in pixels. 4096² is safely within every
/// wgpu-supported device's texture limits.
const ATLAS_MAX_SIZE: u32 = 4096;
/// 1px padding between glyphs to avoid bleeding at UV edges.
const PAD: u32 = 1;
#[cfg(target_os = "macos")]
pub const UI_FONT_FAMILY: &str = ".SF NS";
#[cfg(not(target_os = "macos"))]
pub const UI_FONT_FAMILY: &str = "Yu Gothic UI";
const LABEL_FONT_SIZE: f32 = 14.0;
const LABEL_LINE_HEIGHT: f32 = 18.0;
const LABEL_LAYOUT_CACHE_CAPACITY: usize = 4096;
/// Strong, soft-edged shadow in logical px: (x offset, y offset, alpha).
/// Shared by app labels, folder labels, and the open-folder title.
const LABEL_SHADOW_LAYERS: &[(f32, f32, f32)] = &[
    (0.0, 1.0, 0.48),
    (0.0, 2.0, 0.26),
    (-0.8, 1.3, 0.18),
    (0.8, 1.3, 0.18),
    (0.0, 3.0, 0.12),
];

impl TextRenderer {
    pub fn new() -> Self {
        Self::with_atlas_size(ATLAS_INITIAL_SIZE, ATLAS_INITIAL_SIZE)
    }

    /// Construct with an explicit initial atlas size. Used by tests to force
    /// growth without rasterizing thousands of glyphs.
    fn with_atlas_size(atlas_w: u32, atlas_h: u32) -> Self {
        let font_system = platform_font_system();
        let swash = SwashCache::new();
        let atlas = vec![0u8; (atlas_w * atlas_h * 4) as usize];
        Self {
            font_system,
            swash,
            atlas,
            atlas_w,
            atlas_h,
            cache: HashMap::new(),
            cursor_x: PAD,
            cursor_y: PAD,
            row_height: 0,
            atlas_dirty: true,
            atlas_grew: false,
            label_layout_cache: HashMap::new(),
        }
    }

    pub fn atlas_rgba(&self) -> &[u8] {
        &self.atlas
    }

    pub fn atlas_dimensions(&self) -> (u32, u32) {
        (self.atlas_w, self.atlas_h)
    }

    /// Returns true (and clears the flag) if the atlas grew since the last
    /// call. On growth every previously built glyph quad's normalized UVs
    /// are stale (the denominators changed), so the caller must rebuild all
    /// glyph lanes before the next render.
    pub fn take_atlas_grew(&mut self) -> bool {
        std::mem::take(&mut self.atlas_grew)
    }

    /// Lay out all labels and return one `GlyphQuad` per glyph.
    ///
    /// `scale_factor` converts cosmic-text's logical px to physical px (the
    /// units the rest of the renderer uses). Pass the window's scale factor.
    pub fn layout_labels(&mut self, labels: &[Label], scale_factor: f32) -> Vec<GlyphQuad> {
        let mut placed = Vec::new();
        for label in labels {
            let key = LabelLayoutKey {
                text: label.text.clone(),
                max_width_bits: label.max_width.to_bits(),
                scale_factor_bits: scale_factor.to_bits(),
            };
            if !self.label_layout_cache.contains_key(&key) {
                if self.label_layout_cache.len() >= LABEL_LAYOUT_CACHE_CAPACITY {
                    self.label_layout_cache.clear();
                }
                let relative = self
                    .layout_phase(
                        &[Label {
                            text: label.text.clone(),
                            x: 0.0,
                            y: 0.0,
                            max_width: label.max_width,
                            color: [1.0; 4],
                        }],
                        scale_factor,
                    )
                    .into_iter()
                    .map(|glyph| CachedLabelGlyph {
                        physical: glyph.physical,
                        x: glyph.x,
                        y: glyph.y,
                    })
                    .collect();
                self.label_layout_cache.insert(key.clone(), relative);
            }
            if let Some(relative) = self.label_layout_cache.get(&key) {
                placed.extend(relative.iter().map(|glyph| PlacedGlyph {
                    physical: glyph.physical.clone(),
                    x: label.x + glyph.x,
                    y: label.y + glyph.y,
                    color: label.color,
                }));
            }
        }
        self.raster_phase(placed, scale_factor, LABEL_SHADOW_LAYERS)
    }

    /// Lay out a single centered line of text with an explicit color, returning
    /// glyph quads *without* the label drop-shadow. Used by the bottom control
    /// (search pill label + search field query + placeholder), which draws its
    /// own crisp text over the Liquid Glass capsule.
    ///
    /// `spec.center` is the on-screen center of the line in physical px. The
    /// glyph quads are positioned so the line is horizontally centered on it.
    pub fn layout_centered_line(&mut self, spec: &CenteredLineSpec<'_>) -> Vec<GlyphQuad> {
        self.layout_centered_line_weighted(spec, Weight::NORMAL)
    }

    /// Same as [`layout_centered_line`] but also returns the laid-out line
    /// width in physical px. Lets callers that need both the quads and the
    /// measurement (e.g. the context menu, which centers the label inside a
    /// row) do it in a single shaping pass instead of calling
    /// [`measure_text`] first.
    pub fn layout_centered_line_with_width(
        &mut self,
        spec: &CenteredLineSpec<'_>,
    ) -> (Vec<GlyphQuad>, f32) {
        self.layout_centered_line_weighted_with_width(spec, Weight::NORMAL)
    }

    /// Weighted variant used by semantic UI text such as a folder title.
    pub fn layout_centered_line_weighted(
        &mut self,
        spec: &CenteredLineSpec<'_>,
        weight: Weight,
    ) -> Vec<GlyphQuad> {
        self.layout_centered_line_weighted_with_layers(spec, weight, &[])
            .0
    }

    /// Centered semantic text with the same soft layered shadow used by app
    /// labels. Folder titles use this so they retain contrast over the moving
    /// blurred scene without changing their bold shaping or fitting.
    pub fn layout_centered_line_weighted_with_shadow(
        &mut self,
        spec: &CenteredLineSpec<'_>,
        weight: Weight,
    ) -> Vec<GlyphQuad> {
        self.layout_centered_line_weighted_with_layers(spec, weight, LABEL_SHADOW_LAYERS)
            .0
    }

    fn layout_centered_line_weighted_with_layers(
        &mut self,
        spec: &CenteredLineSpec<'_>,
        weight: Weight,
        shadow_layers: &[(f32, f32, f32)],
    ) -> (Vec<GlyphQuad>, f32) {
        let CenteredLineSpec {
            text,
            font_size,
            line_height,
            family,
            color,
            center,
            scale_factor,
        } = *spec;
        let metrics = Metrics::new(font_size, line_height);
        let attrs = Attrs::new()
            .family(Family::Name(family))
            .weight(weight)
            .color(Color::rgba(
                (color[0] * 255.0).round() as u8,
                (color[1] * 255.0).round() as u8,
                (color[2] * 255.0).round() as u8,
                (color[3] * 255.0).round() as u8,
            ));
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        // No wrapping: the control text is short.
        buffer.set_wrap(Wrap::None);
        buffer.set_size(Some(f32::MAX / 4.0), Some(line_height * 2.0 / scale_factor));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut placed = Vec::new();
        let mut line_w = 0.0f32;
        let baseline_y = center.1 - line_height * 0.5 * scale_factor;
        // Single line only: take the first layout run.
        if let Some(run) = buffer.layout_runs().next() {
            line_w = run.line_w * scale_factor;
            let centered_x = (center.0 / scale_factor - run.line_w * 0.5).max(0.0);
            // Round the physical origin: `CacheKey` bins glyphs by subpixel
            // position, so an animated fractional origin would rasterize up
            // to 4 atlas entries per glyph. Snapping to whole physical px
            // keeps one entry per glyph and is visually indistinguishable.
            let line_origin = (
                (centered_x * scale_factor).round(),
                (baseline_y + run.line_y * scale_factor).round(),
            );
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical(line_origin, scale_factor);
                let x = physical.x as f32;
                let y = physical.y as f32;
                placed.push(PlacedGlyph {
                    physical,
                    x,
                    y,
                    color,
                });
            }
        }
        let quads = self.raster_phase(placed, scale_factor, shadow_layers);
        (quads, line_w)
    }

    fn layout_centered_line_weighted_with_width(
        &mut self,
        spec: &CenteredLineSpec<'_>,
        weight: Weight,
    ) -> (Vec<GlyphQuad>, f32) {
        self.layout_centered_line_weighted_with_layers(spec, weight, &[])
    }

    /// Lay out a single line anchored to a left edge, returning the glyph
    /// quads and the line width in physical px in a single shaping pass.
    ///
    /// `left` is the on-screen left edge of the line (physical px), `center_y`
    /// the vertical center. Unlike [`layout_centered_line`] this needs no prior
    /// `measure_text` to position the text, so callers that left-align a label
    /// inside a known row (e.g. the context menu) shape once instead of twice.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_left_anchored_line_with_width(
        &mut self,
        text: &str,
        left: f32,
        center_y: f32,
        font_size: f32,
        line_height: f32,
        family: &str,
        color: [f32; 4],
        scale_factor: f32,
    ) -> (Vec<GlyphQuad>, f32) {
        let metrics = Metrics::new(font_size, line_height);
        let attrs = Attrs::new()
            .family(Family::Name(family))
            .weight(Weight::NORMAL)
            .color(Color::rgba(
                (color[0] * 255.0).round() as u8,
                (color[1] * 255.0).round() as u8,
                (color[2] * 255.0).round() as u8,
                (color[3] * 255.0).round() as u8,
            ));
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(Wrap::None);
        buffer.set_size(Some(f32::MAX / 4.0), Some(line_height * 2.0 / scale_factor));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut placed = Vec::new();
        let mut line_w = 0.0f32;
        let baseline_y = center_y - line_height * 0.5 * scale_factor;
        if let Some(run) = buffer.layout_runs().next() {
            line_w = run.line_w * scale_factor;
            // `left` is the line's left edge in physical px. Round it like the
            // centered path does so `CacheKey` bins glyphs at whole physical px
            // (one atlas entry per glyph, no subpixel proliferation).
            let line_origin = (
                left.round(),
                (baseline_y + run.line_y * scale_factor).round(),
            );
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical(line_origin, scale_factor);
                let x = physical.x as f32;
                let y = physical.y as f32;
                placed.push(PlacedGlyph {
                    physical,
                    x,
                    y,
                    color,
                });
            }
        }
        let quads = self.raster_phase(placed, scale_factor, &[]);
        (quads, line_w)
    }

    /// Measure a single line of text's laid-out width in physical px without
    /// rasterizing it into the atlas. Runs the *same* cosmic-text shaping as
    /// [`layout_centered_line`] so the result matches what will be drawn
    /// (ASCII / CJK / ligatures all accounted for). Returns 0.0 on an empty
    /// or unshapable string.
    pub fn measure_text(&mut self, spec: &CenteredLineSpec<'_>) -> f32 {
        self.measure_text_weighted(spec, Weight::NORMAL)
    }

    pub fn measure_text_weighted(&mut self, spec: &CenteredLineSpec<'_>, weight: Weight) -> f32 {
        let CenteredLineSpec {
            text,
            font_size,
            line_height,
            family,
            scale_factor,
            ..
        } = *spec;
        let metrics = Metrics::new(font_size, line_height);
        let attrs = Attrs::new().family(Family::Name(family)).weight(weight);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(Wrap::None);
        buffer.set_size(Some(f32::MAX / 4.0), Some(line_height * 2.0 / scale_factor));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        match buffer.layout_runs().next() {
            // line_w is in logical px → physical px.
            Some(run) => run.line_w * scale_factor,
            None => 0.0,
        }
    }

    // -- Phase 1: cosmic-text layout --------------------------------------

    fn layout_phase(&mut self, labels: &[Label], scale_factor: f32) -> Vec<PlacedGlyph> {
        let metrics = Metrics::new(LABEL_FONT_SIZE, LABEL_LINE_HEIGHT);
        let attrs = Attrs::new()
            .family(Family::Name(UI_FONT_FAMILY))
            .color(Color::rgba(255, 255, 255, 255));

        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(Wrap::WordOrGlyph);

        let mut out = Vec::new();

        for label in labels {
            // cosmic-text lays out in logical px; we scale to physical.
            buffer.set_size(
                Some(label.max_width / scale_factor),
                // Metrics and Buffer dimensions are both logical pixels.
                // The label rectangle is physical (hence the width divide),
                // but the two-line logical height must not be divided by the
                // display scale a second time. Doing so collapsed Retina
                // labels to one line.
                Some(LABEL_LINE_HEIGHT * 2.0),
            );
            buffer.set_text(&label.text, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);

            // Each layout run == one wrapped line. Cap at 2 lines.
            for (line_i, run) in buffer.layout_runs().enumerate() {
                if line_i >= 2 {
                    break;
                }
                let label_width = label.max_width / scale_factor;
                let centered_x = ((label_width - run.line_w) * 0.5).max(0.0);
                let line_origin = (
                    label.x + centered_x * scale_factor,
                    label.y + run.line_y * scale_factor,
                );
                for glyph in run.glyphs.iter() {
                    let physical = glyph.physical(line_origin, scale_factor);
                    let x = physical.x as f32;
                    let y = physical.y as f32;
                    out.push(PlacedGlyph {
                        physical,
                        x,
                        y,
                        color: label.color,
                    });
                }
            }
        }

        out
    }

    // -- Phase 2: rasterize into the atlas, emit quads --------------------

    fn raster_phase(
        &mut self,
        placed: Vec<PlacedGlyph>,
        scale_factor: f32,
        shadow_layers: &[(f32, f32, f32)],
    ) -> Vec<GlyphQuad> {
        // Ensure every glyph is in the atlas *before* computing UVs: a
        // mid-batch atlas grow changes the UV denominators, and computing
        // them per-glyph would leave earlier quads in this batch stale.
        let mut entries = Vec::with_capacity(placed.len());
        for g in placed {
            let entry = match self.ensure_glyph(&g.physical) {
                Some(e) => e,
                None => continue,
            };
            entries.push((entry, g));
        }

        let (aw, ah) = (self.atlas_w as f32, self.atlas_h as f32);
        let mut glyphs = Vec::with_capacity(entries.len());
        for (entry, g) in entries {
            // The bitmap's top-left relative to the pen position:
            //   x = pen_x + placement.left
            //   y = pen_y - placement.top   (swash Y is up-positive)
            let bx = g.x + entry.off_x as f32;
            let by = g.y - entry.off_y as f32;
            glyphs.push(GlyphQuad {
                x: bx,
                y: by,
                w: entry.w as f32,
                h: entry.h as f32,
                u0: entry.x as f32 / aw,
                v0: entry.y as f32 / ah,
                u1: (entry.x + entry.w) as f32 / aw,
                v1: (entry.y + entry.h) as f32 / ah,
                color: g.color,
                clip_rect: [0.0; 4],
                clip_radius: [0.0; 4],
            });
        }

        let mut quads = Vec::with_capacity(glyphs.len() * (shadow_layers.len() + 1));
        for glyph in glyphs.iter().copied() {
            for &(dx, dy, alpha) in shadow_layers {
                quads.push(glyph.with_offset_and_color(
                    dx * scale_factor,
                    dy * scale_factor,
                    [0.0, 0.0, 0.0, alpha * glyph.color[3]],
                ));
            }
        }
        quads.extend(glyphs);
        quads
    }

    /// Ensure a glyph is in the atlas (rasterize on miss). Returns its entry.
    fn ensure_glyph(&mut self, physical: &PhysicalGlyph) -> Option<AtlasEntry> {
        if let Some(&e) = self.cache.get(&physical.cache_key) {
            return Some(e);
        }

        // Rasterize and copy the bits we need out of the cache, so the
        // mutable borrow of `self.swash` ends before we touch `self.atlas`.
        let (content, data, placement) = {
            let image = self
                .swash
                .get_image(&mut self.font_system, physical.cache_key);
            let image = image.as_ref()?;
            (image.content, image.data.clone(), image.placement)
        };

        let w = placement.width;
        let h = placement.height;
        if w == 0 || h == 0 {
            return None;
        }

        // A glyph that cannot fit even at the maximum atlas size is dropped.
        if w + 2 * PAD > ATLAS_MAX_SIZE || h + 2 * PAD > ATLAS_MAX_SIZE {
            return None;
        }
        // Grow until the glyph fits within the atlas width (pathological
        // ultra-wide glyphs only; normal UI text never triggers this).
        while w + 2 * PAD > self.atlas_w {
            if !self.grow_atlas() {
                return None;
            }
        }
        // Find a slot in the current row, wrapping to a new row if needed.
        if self.cursor_x + w + PAD > self.atlas_w {
            self.cursor_y += self.row_height + PAD;
            self.cursor_x = PAD;
            self.row_height = 0;
        }
        // Out of rows: grow the atlas instead of evicting. Existing glyphs
        // keep their pixel positions, so retained quads never point at
        // repurposed texels — only the UV denominators change, which the
        // app layer handles via `take_atlas_grew`.
        while self.cursor_y + h + PAD > self.atlas_h {
            if !self.grow_atlas() {
                // Already at the maximum size; drop the glyph.
                return None;
            }
        }

        let dst_x = self.cursor_x;
        let dst_y = self.cursor_y;
        self.row_height = self.row_height.max(h);
        self.cursor_x += w + PAD;

        self.blit(content, &data, w, h, dst_x, dst_y);

        let entry = AtlasEntry {
            x: dst_x,
            y: dst_y,
            w,
            h,
            off_x: placement.left,
            off_y: placement.top,
        };
        self.cache.insert(physical.cache_key, entry);
        self.atlas_dirty = true;
        Some(entry)
    }

    /// Double the atlas (capped at [`ATLAS_MAX_SIZE`]), copying every
    /// existing row to the same pixel position in the new buffer. Cache
    /// entries and the row-packer cursor stay valid as-is. Returns false if
    /// the atlas is already at its maximum size.
    fn grow_atlas(&mut self) -> bool {
        if self.atlas_w >= ATLAS_MAX_SIZE && self.atlas_h >= ATLAS_MAX_SIZE {
            return false;
        }
        let new_w = (self.atlas_w * 2).min(ATLAS_MAX_SIZE);
        let new_h = (self.atlas_h * 2).min(ATLAS_MAX_SIZE);
        let mut new_atlas = vec![0u8; (new_w * new_h * 4) as usize];
        let old_row_bytes = (self.atlas_w * 4) as usize;
        for y in 0..self.atlas_h {
            let src = (y * self.atlas_w * 4) as usize;
            let dst = (y * new_w * 4) as usize;
            new_atlas[dst..dst + old_row_bytes]
                .copy_from_slice(&self.atlas[src..src + old_row_bytes]);
        }
        self.atlas = new_atlas;
        self.atlas_w = new_w;
        self.atlas_h = new_h;
        self.atlas_dirty = true;
        self.atlas_grew = true;
        true
    }

    /// Copy a swash image into the RGBA atlas, normalizing Mask/Color forms.
    fn blit(
        &mut self,
        content: cosmic_text::SwashContent,
        data: &[u8],
        w: u32,
        h: u32,
        dst_x: u32,
        dst_y: u32,
    ) {
        use cosmic_text::SwashContent;
        match content {
            SwashContent::Mask => {
                // Single-channel alpha → white glyph with coverage alpha.
                for y in 0..h {
                    for x in 0..w {
                        let a = data[(y * w + x) as usize];
                        self.write_pixel(dst_x + x, dst_y + y, 255, 255, 255, a);
                    }
                }
            }
            SwashContent::SubpixelMask => {
                let mut i = 0;
                for y in 0..h {
                    for x in 0..w {
                        let r = data[i] as u16;
                        let g = data[i + 1] as u16;
                        let b = data[i + 2] as u16;
                        let a = ((r + g + b) / 3) as u8;
                        self.write_pixel(dst_x + x, dst_y + y, 255, 255, 255, a);
                        i += 4;
                    }
                }
            }
            SwashContent::Color => {
                // Color emoji: BGRA → RGBA.
                let mut i = 0;
                for y in 0..h {
                    for x in 0..w {
                        let b = data[i];
                        let g = data[i + 1];
                        let r = data[i + 2];
                        let a = data[i + 3];
                        self.write_pixel(dst_x + x, dst_y + y, r, g, b, a);
                        i += 4;
                    }
                }
            }
        }
    }

    #[inline]
    fn write_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
        let idx = ((y * self.atlas_w + x) * 4) as usize;
        let px = &mut self.atlas[idx..idx + 4];
        px[0] = r;
        px[1] = g;
        px[2] = b;
        px[3] = a;
    }
}

#[cfg(target_os = "macos")]
fn platform_font_system() -> FontSystem {
    let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
    macos_font_system_for_locale(&locale)
}

#[cfg(target_os = "macos")]
fn macos_font_system_for_locale(locale: &str) -> FontSystem {
    // B.3 instrumentation: isolate the cost of font DB construction (file I/O
    // over every installed font) from the rest of TextRenderer::new.
    let timer = crate::startup_timer::get();
    timer.mark(crate::startup_timer::prefix::STARTUP, "font load start");
    let mut db = cosmic_text::fontdb::Database::new();
    db.load_system_fonts();
    db.set_sans_serif_family(UI_FONT_FAMILY);
    db.set_serif_family("New York");
    db.set_monospace_family("Menlo");
    timer.mark(
        crate::startup_timer::prefix::STARTUP,
        "font load end (FontSystem built)",
    );

    FontSystem::new_with_locale_and_db(macos_fallback_locale(locale), db)
}

#[cfg(not(target_os = "macos"))]
fn platform_font_system() -> FontSystem {
    // B.3 instrumentation: FontSystem::new() itself performs load_system_fonts.
    let timer = crate::startup_timer::get();
    timer.mark(crate::startup_timer::prefix::STARTUP, "font load start");
    let system = FontSystem::new();
    timer.mark(
        crate::startup_timer::prefix::STARTUP,
        "font load end (FontSystem built)",
    );
    system
}

#[cfg(target_os = "macos")]
fn macos_fallback_locale(locale: &str) -> String {
    let normalized = locale.replace('_', "-");
    let mut subtags = normalized.split('-');
    match subtags
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "ja" => "ja".to_owned(),
        "ko" => "ko".to_owned(),
        "zh" => {
            let region = subtags.find_map(|subtag| {
                let upper = subtag.to_ascii_uppercase();
                matches!(upper.as_str(), "HK" | "TW").then_some(upper)
            });
            match region.as_deref() {
                Some("HK") => "zh-HK".to_owned(),
                Some("TW") => "zh-TW".to_owned(),
                _ => "zh-CN".to_owned(),
            }
        }
        _ => normalized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(text: &str, x: f32) -> Label {
        Label {
            text: text.to_owned(),
            x,
            y: 40.0,
            max_width: 280.0,
            color: [1.0; 4],
        }
    }

    #[test]
    fn grow_preserves_existing_glyph_pixel_positions_and_uvs_track_it() {
        // Use a tiny atlas so a handful of glyphs forces a grow.
        let mut renderer = TextRenderer::with_atlas_size(64, 64);
        // Lay out enough distinct text to force growth.
        let labels: Vec<Label> = (0..40)
            .map(|i| Label {
                text: format!("App number {i} with a long name"),
                x: 0.0,
                y: 0.0,
                max_width: 280.0,
                color: [1.0; 4],
            })
            .collect();
        let _quads_a = renderer.layout_labels(&labels, 2.0);
        let grew = renderer.take_atlas_grew();
        let after_dims = renderer.atlas_dimensions();
        assert!(grew, "the labels should have forced a grow");
        assert!(
            after_dims.0 > 64 && after_dims.1 > 64,
            "atlas should have grown"
        );

        // Re-lay out the SAME labels again (simulates a relayout after grow).
        // UVs must now be computed against the grown dimensions and reference
        // valid, integer-pixel-aligned texels within the current atlas.
        let quads_b = renderer.layout_labels(&labels, 2.0);
        let (aw, _ah) = renderer.atlas_dimensions();
        for q in &quads_b {
            assert!(q.u0 >= 0.0 && q.u1 <= 1.0, "u out of range: {:?}", q);
            assert!(q.v0 >= 0.0 && q.v1 <= 1.0, "v out of range: {:?}", q);
            let px0 = (q.u0 * aw as f32).round() as i32;
            let px1 = (q.u1 * aw as f32).round() as i32;
            assert!(
                px0 >= 0 && px1 <= aw as i32 && px1 > px0,
                "px u: {px0}..{px1}"
            );
        }
    }

    /// After a grow, the retained Grid labels — when re-laid-out as
    /// `tick_frame` does — must still resolve to non-empty atlas pixels.
    /// A quad pointing at an empty texel would render the label invisibly
    /// (the originally reported "text vanishes" symptom). Grow copies every
    /// existing glyph to the same pixel position, so the re-laid-out quads
    /// (with smaller normalized UVs) must land on the same ink.
    #[test]
    fn grow_keeps_retained_grid_uv_pointing_at_rasterized_pixels() {
        let mut renderer = TextRenderer::with_atlas_size(128, 128);

        // Phase 1: lay out grid labels (the retained lane).
        let grid_labels: Vec<Label> = (0..20)
            .map(|i| Label {
                text: format!("Grid app {i} ァィゥェォ"),
                x: 0.0,
                y: 0.0,
                max_width: 280.0,
                color: [1.0; 4],
            })
            .collect();
        let _grid_quads_initial = renderer.layout_labels(&grid_labels, 2.0);
        let (iw, ih) = renderer.atlas_dimensions();

        // Phase 2: rasterize MORE distinct glyphs (transient lanes: search
        // query, settings, etc.) to force at least one grow.
        let extra_labels: Vec<Label> = (0..60)
            .map(|i| Label {
                text: format!("Search result №{i} αβγδ ①②③④⑤"),
                x: 0.0,
                y: 0.0,
                max_width: 280.0,
                color: [1.0; 4],
            })
            .collect();
        let _extra = renderer.layout_labels(&extra_labels, 2.0);
        assert!(
            renderer.take_atlas_grew(),
            "the extra labels should have forced a grow"
        );
        let (gw, gh) = renderer.atlas_dimensions();
        assert!(gw > iw || gh > ih, "atlas should have grown");

        // Phase 3: simulate `tick_frame`'s post-grow relayout — re-lay-out the
        // retained grid labels against the current (grown) dimensions.
        let grid_quads_after = renderer.layout_labels(&grid_labels, 2.0);

        // Each grid quad must map to at least one non-zero-alpha texel. A glyph
        // is thin, so scan the whole UV rect rather than just its center.
        for (i, q) in grid_quads_after.iter().enumerate() {
            let x0 = (q.u0 * gw as f32) as u32;
            let y0 = (q.v0 * gh as f32) as u32;
            let x1 = ((q.u1 * gw as f32) as u32).min(gw);
            let y1 = ((q.v1 * gh as f32) as u32).min(gh);
            let found_alpha = (y0..y1).any(|yy| {
                (x0..x1).any(|xx| {
                    let idx = ((yy * gw + xx) * 4) as usize;
                    renderer.atlas_rgba().get(idx + 3).copied().unwrap_or(0) > 0
                })
            });
            assert!(
                found_alpha,
                "grid quad {i} at uv=({},{},{},{}) -> px({x0},{y0})-({x1},{y1}) \
                 points only at empty texels — label would vanish after grow",
                q.u0, q.v0, q.u1, q.v1
            );
        }
    }

    #[test]
    fn label_layout_cache_reuses_two_line_shaping_across_positions() {
        let mut renderer = TextRenderer::new();
        let first = renderer.layout_labels(&[label("Adobe Premiere Pro 2026", 20.0)], 2.0);
        assert_eq!(renderer.label_layout_cache.len(), 1);

        let cached = renderer
            .label_layout_cache
            .values()
            .next()
            .expect("label layout should be cached");
        let first_line_y = cached.first().expect("label should contain glyphs").y;
        assert!(
            cached
                .iter()
                .any(|glyph| (glyph.y - first_line_y).abs() > LABEL_LINE_HEIGHT),
            "a long Mac app name should use the second label line"
        );

        let second = renderer.layout_labels(&[label("Adobe Premiere Pro 2026", 140.0)], 2.0);
        assert_eq!(renderer.label_layout_cache.len(), 1);
        assert_eq!(first.len(), second.len());
        for (before, after) in first.iter().zip(&second) {
            assert!((after.x - before.x - 120.0).abs() < 0.01);
            assert!((after.y - before.y).abs() < 0.01);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_locale_uses_region_appropriate_han_fallback() {
        assert_eq!(macos_fallback_locale("ja-JP"), "ja");
        assert_eq!(macos_fallback_locale("ja_JP"), "ja");
        assert_eq!(macos_fallback_locale("ko-KR"), "ko");
        assert_eq!(macos_fallback_locale("zh-Hant-TW"), "zh-TW");
        assert_eq!(macos_fallback_locale("zh-Hant-HK"), "zh-HK");
        assert_eq!(macos_fallback_locale("zh-Hans-CN"), "zh-CN");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_japanese_text_uses_hiragino_instead_of_simplified_chinese() {
        let mut font_system = macos_font_system_for_locale("ja-JP");
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(24.0, 30.0));
        buffer.set_text(
            "制作とコミュニケーション",
            &Attrs::new().family(Family::Name(UI_FONT_FAMILY)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);

        let family_names: Vec<_> = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .filter_map(|glyph| font_system.db().face(glyph.font_id))
            .flat_map(|face| face.families.iter().map(|(name, _)| name.as_str()))
            .collect();
        assert!(family_names.contains(&"Hiragino Sans"));
        assert!(!family_names.contains(&"PingFang SC"));
    }
}
