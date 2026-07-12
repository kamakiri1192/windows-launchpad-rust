//! Procedural overlay instance buffers: bottom control, corner gear, and the
//! settings overlay (which reuses the control pipelines).
//!
//! Each buffer is capacity-managed ([`InstanceBuffer`]): an empty list sets
//! the logical draw count to zero (the pass skips it) but keeps the buffer
//! allocated for reuse, so a surface that disappears and reappears does not
//! churn allocations. The `ControlUniforms` struct is the small
//! viewport/scroll/frame uniform shared by the control shape and text shaders.

// ---- overlay instance data (mirrors shader_control.wgsl) --------------------

/// One drawable overlay element for the bottom control. Matches the WGSL
/// `@location(0..3)` instance attributes of `shader_control.wgsl`. Built by
/// `build_overlay_instances` from a resolved geometry + layer list.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ControlInstance {
    /// Element center in physical px.
    pub center: [f32; 2],
    /// (size/radius, alpha, extra, _pad).
    pub params: [f32; 4],
    /// RGBA tint (non-premultiplied).
    pub color: [f32; 4],
    /// (kind, a, b, c) element-specific payload.
    pub kind: [f32; 4],
}

impl ControlInstance {
    pub const ATTRIBS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ControlInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ControlInstance::ATTRIBS,
    };
}

/// Element kind values matching `shader_control.wgsl`.
pub const KIND_MAGNIFIER: f32 = 0.0;
pub const KIND_DOT: f32 = 1.0;
pub const KIND_CARET: f32 = 2.0;
/// Close button (×). Public so the settings panel can draw one too.
pub const KIND_CLOSE: f32 = 3.0;
/// Settings gear (ring + radial teeth). Drawn frame-independent, so unlike the
/// edit badge (kind 4) it is neither scroll-coupled nor frame-masked.
pub const KIND_GEAR: f32 = 5.0;
/// Rounded rectangle ink/fill used by the settings panel.
pub const KIND_ROUND_RECT: f32 = 6.0;
/// Check mark used by the settings panel's selected rows.
pub const KIND_CHECK: f32 = 7.0;
/// Chevron used by settings action rows.
pub const KIND_CHEVRON: f32 = 8.0;

/// Uniform for the bottom-control overlay + text shaders. The bottom control
/// uses only the viewport; edit badges also use scroll and the page frame clip.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ControlUniforms {
    pub(super) viewport_scroll: [f32; 4],
    pub(super) frame_center_radius: [f32; 4],
    pub(super) frame_half_size: [f32; 4],
}
