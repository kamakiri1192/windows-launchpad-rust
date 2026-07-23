//! Text rendering for the Launchpad MVP.
//!
//! Uses `cosmic-text` to shape/layout each label (Japanese-capable) and
//! `SwashCache` to rasterize glyphs into a CPU-side texture atlas. The atlas
//! is uploaded once to the GPU; the renderer instance-draws one quad per
//! glyph, sampling the atlas.
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
/// pipeline. 64 bytes: 16 for xywh, 16 for the atlas UV rect, 16 for the
/// non-premultiplied RGBA tint, and a trailing 16-byte `extra` vec that the
/// fragment shader reads to switch between SDF and plain RGBA glyphs and to
/// carry the per-instance SDF spread.
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
    /// Shader-facing payload:
    /// - `extra[0]`: 1.0 for an SDF glyph (distance field in the alpha/red
    ///   channel), 0.0 for a plain RGBA glyph (e.g. colour emoji).
    /// - `extra[1]`: SDF spread in physical px used to decode the sampled
    ///   distance back into pixels. Unused for plain glyphs.
    /// - `extra[2..]`: reserved (padding for the vec4 alignment).
    pub extra: [f32; 4],
}

impl GlyphQuad {
    pub const ATTRIBS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GlyphQuad>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &GlyphQuad::ATTRIBS,
    };
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
    /// True when the stored pixels are an SDF (distance field) rather than a
    /// plain coverage/colour bitmap. The fragment shader branches on this.
    is_sdf: bool,
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
    /// Atlas RGBA buffer (CPU side), row-major, `ATLAS_W * ATLAS_H * 4` bytes.
    atlas: Vec<u8>,
    /// Cache key → atlas placement.
    cache: HashMap<cosmic_text::CacheKey, AtlasEntry>,
    /// Next free cell cursor for the row packer.
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    /// True if the atlas changed since the last GPU upload.
    pub atlas_dirty: bool,
    /// Shaping is independent of a label's on-screen position. Folder paging
    /// changes only that position, so retain relative glyph layouts instead
    /// of asking cosmic-text to shape every visible name on every frame.
    label_layout_cache: HashMap<LabelLayoutKey, Vec<CachedLabelGlyph>>,
}

/// Atlas grown to 2048² to accommodate the SDF padding border added around
/// every mask glyph (±`SDF_PADDING` px per side).
const ATLAS_W: u32 = 2048;
const ATLAS_H: u32 = 2048;
/// Padding between glyphs in the atlas to avoid bleeding at UV edges.
const PAD: u32 = 1;
const LABEL_FONT_FAMILY: &str = "Yu Gothic UI";
const LABEL_FONT_SIZE: f32 = 14.0;
const LABEL_LINE_HEIGHT: f32 = 18.0;
const LABEL_LAYOUT_CACHE_CAPACITY: usize = 4096;
/// SDF spread in physical px: the maximum distance (in either direction)
/// encoded around each glyph outline. ±`SDF_SPREAD` maps to the atlas byte
/// range 0..=255, so the fragment shader reconstructs the pixel distance as
/// `(sampled * 2.0 - 1.0) * spread`. A spread of 4 px comfortably covers the
/// CSS reference (`1px 1px 2px` main shadow + `0 0 4px` halo) at 1× and
/// leaves headroom for Retina.
const SDF_SPREAD: f32 = 4.0;
/// Per-glyph border added by [`oxitext_sdf::compute_sdf`] so the distance
/// field can represent the full ±`SDF_SPREAD` range outside the outline
/// without clipping at the tile edge. Equals `ceil(SDF_SPREAD)`.
const SDF_PADDING: u32 = 4;

impl TextRenderer {
    pub fn new() -> Self {
        let font_system = FontSystem::new();
        let swash = SwashCache::new();
        let atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        Self {
            font_system,
            swash,
            atlas,
            cache: HashMap::new(),
            cursor_x: PAD,
            cursor_y: PAD,
            row_height: 0,
            atlas_dirty: true,
            label_layout_cache: HashMap::new(),
        }
    }

    pub fn atlas_rgba(&self) -> &[u8] {
        &self.atlas
    }

    pub const fn atlas_dimensions() -> (u32, u32) {
        (ATLAS_W, ATLAS_H)
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
        self.raster_phase(placed)
    }

    /// Lay out a single centered line of text with an explicit color. Used by
    /// the bottom control (search pill label + search field query +
    /// placeholder) and by semantic UI text such as a folder title. Shadows
    /// are no longer baked in here; the text fragment shader derives them from
    /// the SDF distance field, so all centered lines share one crisp code path.
    ///
    /// `spec.center` is the on-screen center of the line in physical px. The
    /// glyph quads are positioned so the line is horizontally centered on it.
    pub fn layout_centered_line(&mut self, spec: &CenteredLineSpec<'_>) -> Vec<GlyphQuad> {
        self.layout_centered_line_weighted(spec, Weight::NORMAL)
    }

    /// Weighted variant used by semantic UI text such as a folder title.
    pub fn layout_centered_line_weighted(
        &mut self,
        spec: &CenteredLineSpec<'_>,
        weight: Weight,
    ) -> Vec<GlyphQuad> {
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
        let baseline_y = center.1 - line_height * 0.5 * scale_factor;
        // Single line only: take the first layout run.
        if let Some(run) = buffer.layout_runs().next() {
            let run_w = run.line_w;
            let centered_x = (center.0 / scale_factor - run_w * 0.5).max(0.0);
            let line_origin = (
                centered_x * scale_factor,
                baseline_y + run.line_y * scale_factor,
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
        self.raster_phase(placed)
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
            .family(Family::Name(LABEL_FONT_FAMILY))
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

    fn raster_phase(&mut self, placed: Vec<PlacedGlyph>) -> Vec<GlyphQuad> {
        let mut quads = Vec::with_capacity(placed.len());
        for g in placed {
            let entry = match self.ensure_glyph(&g.physical) {
                Some(e) => e,
                None => continue,
            };
            // The bitmap's top-left relative to the pen position:
            //   x = pen_x + placement.left
            //   y = pen_y - placement.top   (swash Y is up-positive)
            //
            // For SDF glyphs `off_x`/`off_y` already account for the spread
            // padding, so the distance field extends beyond the original
            // glyph box. Plain RGBA glyphs keep the legacy behaviour.
            let bx = g.x + entry.off_x as f32;
            let by = g.y - entry.off_y as f32;
            quads.push(GlyphQuad {
                x: bx,
                y: by,
                w: entry.w as f32,
                h: entry.h as f32,
                u0: entry.x as f32 / ATLAS_W as f32,
                v0: entry.y as f32 / ATLAS_H as f32,
                u1: (entry.x + entry.w) as f32 / ATLAS_W as f32,
                v1: (entry.y + entry.h) as f32 / ATLAS_H as f32,
                color: g.color,
                extra: [if entry.is_sdf { 1.0 } else { 0.0 }, SDF_SPREAD, 0.0, 0.0],
            });
        }
        quads
    }

    /// Ensure a glyph is in the atlas (rasterize on miss). Returns its entry.
    ///
    /// Mask/subpixel-mask glyphs are converted to a signed distance field with
    /// [`oxitext_sdf::compute_sdf`], which adds `2 * SDF_PADDING` px of border
    /// around the original bitmap so the field can represent the full
    /// ±`SDF_SPREAD` falloff outside the outline. Colour glyphs (emoji) bypass
    /// the SDF path and are stored as plain RGBA, since a distance field cannot
    /// preserve their per-pixel colour.
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

        use cosmic_text::SwashContent;
        let is_sdf = matches!(content, SwashContent::Mask | SwashContent::SubpixelMask);
        let (store_w, store_h, sdf_field) = if is_sdf {
            let coverage = coverage_from_mask(&content, &data, w, h);
            match oxitext_sdf::compute_sdf(
                &coverage,
                w as usize,
                h as usize,
                SDF_SPREAD,
                SDF_PADDING,
            ) {
                Ok(field) => (w + 2 * SDF_PADDING, h + 2 * SDF_PADDING, Some(field)),
                Err(err) => {
                    // SDF failures are unexpected (the coverage map is well
                    // formed at this point). Drop the glyph rather than risk a
                    // misaligned fallback; the atlas stays consistent.
                    eprintln!("SDF generation failed for glyph: {err}; dropping");
                    return None;
                }
            }
        } else {
            (w, h, None)
        };

        // Find a slot in the current row, wrapping to a new row if needed.
        if self.cursor_x + store_w + PAD > ATLAS_W {
            self.cursor_y += self.row_height + PAD;
            self.cursor_x = PAD;
            self.row_height = 0;
        }
        if self.cursor_y + store_h + PAD > ATLAS_H {
            eprintln!("text atlas full; glyph dropped");
            return None;
        }

        let dst_x = self.cursor_x;
        let dst_y = self.cursor_y;
        self.row_height = self.row_height.max(store_h);
        self.cursor_x += store_w + PAD;

        if let Some(field) = sdf_field {
            self.blit_sdf(&field, store_w, store_h, dst_x, dst_y);
        } else {
            self.blit_color_rgba(&data, w, h, dst_x, dst_y);
        }

        // For SDF glyphs the stored tile is padded; offset the placement so
        // the original glyph box still lands at pen + placement.left/top.
        let pad = if is_sdf { SDF_PADDING as i32 } else { 0 };
        let entry = AtlasEntry {
            x: dst_x,
            y: dst_y,
            w: store_w,
            h: store_h,
            off_x: placement.left - pad,
            off_y: placement.top + pad,
            is_sdf,
        };
        self.cache.insert(physical.cache_key, entry);
        self.atlas_dirty = true;
        Some(entry)
    }

    /// Write an SDF distance field into the atlas red channel (GBA = 255 so the
    /// shader can treat the sample as a scalar distance).
    fn blit_sdf(&mut self, field: &[u8], w: u32, h: u32, dst_x: u32, dst_y: u32) {
        for y in 0..h {
            for x in 0..w {
                let d = field[(y * w + x) as usize];
                self.write_pixel(dst_x + x, dst_y + y, d, 255, 255, 255);
            }
        }
    }

    /// Copy a colour (emoji) swash image into the RGBA atlas (BGRA → RGBA).
    fn blit_color_rgba(&mut self, data: &[u8], w: u32, h: u32, dst_x: u32, dst_y: u32) {
        let mut i = 0;
        for _y in 0..h {
            for _x in 0..w {
                let b = data[i];
                let g = data[i + 1];
                let r = data[i + 2];
                let a = data[i + 3];
                self.write_pixel(dst_x + _x, dst_y + _y, r, g, b, a);
                i += 4;
            }
        }
    }

    #[inline]
    fn write_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
        let idx = ((y * ATLAS_W + x) * 4) as usize;
        let px = &mut self.atlas[idx..idx + 4];
        px[0] = r;
        px[1] = g;
        px[2] = b;
        px[3] = a;
    }
}

/// Collapse a swash mask/subpixel-mask image to a single-channel coverage map
/// (`width * height` bytes), the input format [`oxitext_sdf::compute_sdf`]
/// expects. Mask images are already one byte per pixel; subpixel masks are
/// RGB(LCD) triplets averaged to luminance.
fn coverage_from_mask(content: &cosmic_text::SwashContent, data: &[u8], w: u32, h: u32) -> Vec<u8> {
    use cosmic_text::SwashContent;
    match content {
        SwashContent::Mask => data.to_vec(),
        SwashContent::SubpixelMask => {
            let n = (w * h) as usize;
            let mut out = Vec::with_capacity(n);
            let mut i = 0;
            for _ in 0..n {
                let r = data[i] as u16;
                let g = data[i + 1] as u16;
                let b = data[i + 2] as u16;
                out.push(((r + g + b) / 3) as u8);
                i += 4;
            }
            out
        }
        // Colour glyphs are never routed through the SDF path.
        SwashContent::Color => Vec::new(),
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

    #[test]
    fn sdf_marks_inside_outside_and_outline_for_a_filled_square() {
        // A 16×16 solid square: the centre must be "inside" (byte > 128),
        // the corners "outside" (< 128), and the byte range covers the full
        // ±SDF_SPREAD falloff that the shader decodes.
        let coverage = vec![255u8; 16 * 16];
        let sdf = oxitext_sdf::compute_sdf(&coverage, 16, 16, SDF_SPREAD, SDF_PADDING)
            .expect("compute_sdf on solid square");
        // The output includes SDF_PADDING border on every side.
        let side = 16 + 2 * SDF_PADDING as usize;
        assert_eq!(sdf.len(), side * side);
        let center = sdf[(side / 2) * side + side / 2];
        assert!(center > 128, "centre should be inside, got {center}");
        let corner = sdf[0];
        assert!(corner < 128, "corner should be outside, got {corner}");
    }

    #[test]
    fn coverage_from_mask_passes_single_channel_through() {
        use cosmic_text::SwashContent;
        let data = vec![0u8, 128, 255, 64];
        let coverage = coverage_from_mask(&SwashContent::Mask, &data, 4, 1);
        assert_eq!(coverage, data);
    }

    #[test]
    fn coverage_from_mask_averages_subpixel_to_luminance() {
        use cosmic_text::SwashContent;
        // BGRA-ish triplets; luminance is (r+g+b)/3 of the first three bytes.
        let data = vec![30u8, 60, 90, 255];
        let coverage = coverage_from_mask(&SwashContent::SubpixelMask, &data, 1, 1);
        assert_eq!(coverage, vec![((30 + 60 + 90) / 3) as u8]);
    }

    #[test]
    fn layout_labels_tags_mask_glyphs_as_sdf_in_extra() {
        // Shaping a plain ASCII label produces mask glyphs; each emitted quad
        // must carry the SDF flag and spread so the shader can branch.
        let mut renderer = TextRenderer::new();
        let quads = renderer.layout_labels(&[label("ABC", 10.0)], 2.0);
        assert!(!quads.is_empty(), "label should produce glyphs");
        assert!(
            quads.iter().all(|q| q.extra[0] >= 0.5),
            "every mask glyph should be tagged SDF"
        );
        assert!(
            quads.iter().all(|q| (q.extra[1] - SDF_SPREAD).abs() < 1e-3),
            "spread should be SDF_SPREAD"
        );
    }

    #[test]
    fn glyph_quad_is_64_bytes_for_clean_gpu_alignment() {
        assert_eq!(std::mem::size_of::<GlyphQuad>(), 64);
    }

    /// Visual debug: dump the SDF glyph atlas to `target/text-atlas-sdf.png`.
    /// Run with `cargo test --release dump_sdf_atlas_visual -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_sdf_atlas_visual() {
        let mut renderer = TextRenderer::new();
        let labels = [
            ("メール", 10.0),
            ("カレンダー", 40.0),
            ("Safari", 70.0),
            ("Adobe Premiere Pro 2026", 100.0),
        ]
        .map(|(text, y)| Label {
            text: text.into(),
            x: 10.0,
            y,
            max_width: 200.0,
            color: [1.0; 4],
        });
        let _quads = renderer.layout_labels(&labels, 2.0);
        let rgba = renderer.atlas_rgba().to_vec();
        if let Some(img) = image::RgbaImage::from_raw(ATLAS_W, ATLAS_H, rgba) {
            let path = std::path::Path::new("target/text-atlas-sdf.png");
            let _ = img.save(path);
            println!("Saved SDF atlas to {}", path.display());
        }
    }
}
