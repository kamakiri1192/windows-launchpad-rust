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
    Attrs, Buffer, Color, Family, FontSystem, Metrics, PhysicalGlyph, Shaping, SwashCache, Wrap,
};

/// A drawable glyph quad, matching the WGSL instance attributes for the text
/// pipeline. 48 bytes for clean GPU alignment.
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
}

impl GlyphQuad {
    pub const ATTRIBS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

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
}

/// Intermediate record from the layout phase.
struct PlacedGlyph {
    physical: PhysicalGlyph,
    /// On-screen glyph origin before applying the raster image placement.
    x: f32,
    y: f32,
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
}

const ATLAS_W: u32 = 1024;
const ATLAS_H: u32 = 1024;
/// 1px padding between glyphs to avoid bleeding at UV edges.
const PAD: u32 = 1;
const LABEL_FONT_FAMILY: &str = "Yu Gothic UI";
const LABEL_FONT_SIZE: f32 = 14.0;
const LABEL_LINE_HEIGHT: f32 = 18.0;
const LABEL_TEXT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// Soft, layered shadow in logical px: (x offset, y offset, alpha).
const LABEL_SHADOW_LAYERS: &[(f32, f32, f32)] = &[
    (0.0, 1.0, 0.30),
    (0.0, 2.0, 0.14),
    (-0.7, 1.2, 0.10),
    (0.7, 1.2, 0.10),
];

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
        let placed = self.layout_phase(labels, scale_factor);
        self.raster_phase(placed, scale_factor)
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
                Some(LABEL_LINE_HEIGHT * 2.0 / scale_factor),
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
                    out.push(PlacedGlyph { physical, x, y });
                }
            }
        }

        out
    }

    // -- Phase 2: rasterize into the atlas, emit quads --------------------

    fn raster_phase(&mut self, placed: Vec<PlacedGlyph>, scale_factor: f32) -> Vec<GlyphQuad> {
        let mut glyphs = Vec::with_capacity(placed.len());
        for g in placed {
            let entry = match self.ensure_glyph(&g.physical) {
                Some(e) => e,
                None => continue,
            };
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
                u0: entry.x as f32 / ATLAS_W as f32,
                v0: entry.y as f32 / ATLAS_H as f32,
                u1: (entry.x + entry.w) as f32 / ATLAS_W as f32,
                v1: (entry.y + entry.h) as f32 / ATLAS_H as f32,
                color: LABEL_TEXT_COLOR,
            });
        }

        let mut quads = Vec::with_capacity(glyphs.len() * (LABEL_SHADOW_LAYERS.len() + 1));
        for glyph in glyphs.iter().copied() {
            for &(dx, dy, alpha) in LABEL_SHADOW_LAYERS {
                quads.push(glyph.with_offset_and_color(
                    dx * scale_factor,
                    dy * scale_factor,
                    [0.0, 0.0, 0.0, alpha],
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

        // Find a slot in the current row, wrapping to a new row if needed.
        if self.cursor_x + w + PAD > ATLAS_W {
            self.cursor_y += self.row_height + PAD;
            self.cursor_x = PAD;
            self.row_height = 0;
        }
        if self.cursor_y + h + PAD > ATLAS_H {
            eprintln!("text atlas full; glyph dropped");
            return None;
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
        let idx = ((y * ATLAS_W + x) * 4) as usize;
        let px = &mut self.atlas[idx..idx + 4];
        px[0] = r;
        px[1] = g;
        px[2] = b;
        px[3] = a;
    }
}
