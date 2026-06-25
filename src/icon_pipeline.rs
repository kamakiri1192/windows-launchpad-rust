//! Per-icon instance data for the icon render pipeline.
//!
//! Each icon is drawn as a unit quad (two triangles) instanced once per tile
//! that has an icon. The instance carries the tile's geometry (so the icon
//! shader can reuse the same rounded-rect mask as the color tiles) plus the
//! UV rect into the shared icon atlas. The fragment shader samples the atlas
//! and masks it to the rounded squircle shape.

/// One drawable icon instance, matching the WGSL `@location(0..1)` instance
/// attributes. 32 bytes for clean GPU alignment.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IconInstance {
    /// Tile geometry, identical to `TileInstance`: top-left, size, radius.
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub radius: f32,
    /// UV rect into the icon atlas (u0, v0, u1, v1) in 0..1.
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl IconInstance {
    /// Vertex attributes describing this struct for `wgpu::VertexBufferLayout`.
    pub const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<IconInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &IconInstance::ATTRIBS,
    };
}
