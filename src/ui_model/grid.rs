//! Renderer-neutral launcher-grid scene inputs.
//!
//! These values describe visible launcher content and motion intent. They do
//! not contain shader layouts or depend on the renderer. Layout and renderer
//! adapters may both consume them without reaching through the binary grid
//! adapter.

use crate::ui_model::geometry::UvRect;

/// Minimal borrowed view of one visible launcher app.
#[derive(Debug, Clone, Copy)]
pub struct GridApp<'a> {
    pub name: &'a str,
    pub uv: Option<UvRect>,
}

/// Renderer-neutral edit/drag animation state for one launcher tile.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TileAnim {
    pub phase: f32,
    pub lift: f32,
    pub scale: f32,
    pub flags: u32,
}

impl TileAnim {
    pub const FLAG_WIGGLE: u32 = 1 << 0;
    pub const FLAG_DRAG: u32 = 1 << 1;

    pub const IDLE: Self = Self {
        phase: 0.0,
        lift: 0.0,
        scale: 1.0,
        flags: 0,
    };

    #[inline]
    pub fn shader_payload(self) -> [f32; 4] {
        [self.phase, self.lift, self.scale, self.flags as f32]
    }
}
