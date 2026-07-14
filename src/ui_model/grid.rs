//! Renderer-neutral launcher-grid scene inputs.
//!
//! These values describe visible launcher content and motion intent. They do
//! not contain shader layouts or depend on the renderer. Layout and renderer
//! adapters may both consume them without reaching through the binary grid
//! adapter.

use crate::ui_model::geometry::UvRect;

/// Minimal borrowed view of one visible launcher item. A normal app supplies
/// `uv`; a folder-style item supplies up to nine ordered `preview_uvs`.
#[derive(Debug, Clone, Copy)]
pub struct GridItem<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub uv: Option<UvRect>,
    pub preview_uvs: &'a [Option<UvRect>],
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
    /// Fixed screen-space content that bypasses horizontal scrolling and the
    /// page-frame clip (used by generic modal content).
    pub const FLAG_FIXED: u32 = 1 << 2;
    /// Participates in edit motion without exposing the app-only hide badge.
    pub const FLAG_NO_BADGE: u32 = 1 << 3;
    /// Keeps the tile's geometry/motion instance but suppresses its opaque
    /// fallback fill. Generic glass and icon layers can remain visible.
    pub const FLAG_NO_FILL: u32 = 1 << 4;
    /// Treat multiple icon instances as rigid children of one parent pivot.
    /// Used by closed-folder miniatures so the folder, rather than each child,
    /// owns wiggle, scale, and pointer-follow motion.
    pub const FLAG_GROUP_MOTION: u32 = 1 << 5;

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
