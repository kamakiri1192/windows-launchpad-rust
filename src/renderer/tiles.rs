//! Static tile instance buffer and the per-frame viewport/scroll uniform.
//!
//! The uniform struct mirrors the WGSL uniform block declared in
//! `src/shader.wgsl`. Tile instance data is static: it is only rebuilt after a
//! relayout or a tile-data change (reorder / icon load / spring animation), and
//! never on an animation-only frame.

/// One drawable tile, matching the WGSL `@location(0..4)` instance attributes.
/// 48 bytes for clean GPU alignment.
///
/// `extra` carries the edit-mode animation parameters:
/// `(phase, lift, scale, flags)` where flags bit 0 = wiggling and bit 1 = being
/// dragged (lifted + pointer-following, frame clip bypassed).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TileInstance {
    /// Top-left corner of the tile in content pixels.
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub radius: f32,
    /// sRGB-ish color packed as linear RGB in 0..1.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Icon index into the atlas. `-1.0` means "no icon → render the color
    /// tile as a fallback". Otherwise it's the atlas entry index as a float.
    pub icon_index: f32,
    /// Edit-mode animation: `(phase, lift, scale, flags)`.
    pub extra: [f32; 4],
}

impl TileInstance {
    /// Vertex attributes describing this struct for `wgpu::VertexBufferLayout`.
    pub const ATTRIBS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x3,
        3 => Float32,
        4 => Float32x4
    ];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TileInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &TileInstance::ATTRIBS,
    };
}

/// Uniform block mirrored in WGSL.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Uniforms {
    pub(super) viewport: [f32; 2],
    pub(super) scroll_x: f32,
    /// Global animation clock (seconds). Drives the edit-mode wiggle.
    pub(super) time: f32,
    /// Fixed page-frame center in physical px.
    pub(super) frame_center: [f32; 2],
    /// Fixed page-frame half-size in physical px.
    pub(super) frame_half_size: [f32; 2],
    /// Fixed page-frame corner radius in physical px.
    pub(super) frame_radius: f32,
    /// 1.0 while an edit-mode drag is in flight, else 0.0. Tells the dragged
    /// instance's vertex shader to follow `drag_pos` instead of its home cell.
    pub(super) drag_active: f32,
    /// Pointer position (screen px) the dragged icon follows. Only meaningful
    /// while `drag_active` is 1.0.
    pub(super) drag_pos: [f32; 2],
}
