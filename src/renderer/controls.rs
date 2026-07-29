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
/// `@location(0..5)` instance attributes of `shader_control.wgsl`. Built by
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
    /// Clip rectangle in physical px: (min_x, min_y, width, height).
    /// Sentinel: clip_rect.z <= 0.0 means "no clip".
    pub clip_rect: [f32; 4],
    /// Clip corner radius in physical px (0 = sharp corners).
    /// Packed as vec4 with padding: (radius, 0, 0, 0).
    pub clip_radius: [f32; 4],
}

impl ControlInstance {
    pub const ATTRIBS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x4,
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4
    ];

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
/// Slider track (wide rounded bar) used by the settings panel.
pub const KIND_SLIDER_TRACK: f32 = 10.0;
/// Slider knob (filled disk) used by the settings panel.
pub const KIND_SLIDER_KNOB: f32 = 11.0;
/// Reset arrow (↺) used by the settings panel's per-row reset.
pub const KIND_RESET_ICON: f32 = 12.0;
/// Pencil glyph used by the context menu (edit home).
pub const KIND_PENCIL: f32 = 13.0;
/// Eye-with-slash glyph used by the context menu (hide app).
pub const KIND_EYE_OFF: f32 = 14.0;
/// Folder glyph used by the context menu (reveal in Finder/Explorer).
pub const KIND_FOLDER: f32 = 15.0;
/// Plus glyph used by the context menu (larger icon).
pub const KIND_PLUS: f32 = 16.0;
/// Minus glyph used by the context menu (smaller icon).
pub const KIND_MINUS: f32 = 17.0;
/// Info glyph used by the context menu (app info).
pub const KIND_INFO: f32 = 18.0;

/// Uniform for the bottom-control overlay + text shaders. The bottom control
/// uses only the viewport; edit badges also use scroll and the page frame clip.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ControlUniforms {
    pub(super) viewport_scroll: [f32; 4],
    pub(super) frame_center_radius: [f32; 4],
    pub(super) frame_half_size: [f32; 4],
}
