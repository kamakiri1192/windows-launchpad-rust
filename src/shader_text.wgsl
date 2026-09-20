// Text label shaders.
//
// One quad (two triangles) per glyph instance. The quad's screen position is
// offset by the same per-frame `scroll_x` as the tiles, so labels scroll in
// lockstep with icons. The fragment samples the glyph atlas and uses its
// alpha as coverage for each instance tint.

struct Uniforms {
    viewport: vec2<f32>,
    scroll_x: f32,
    // Global animation clock (seconds). Drives the edit-mode wiggle.
    time: f32,
    // Fixed page-frame center (physical px).
    frame_center: vec2<f32>,
    // Fixed page-frame half-size (physical px).
    frame_half_size: vec2<f32>,
    // Fixed page-frame corner radius (physical px).
    frame_radius: f32,
    // 1.0 while an edit-mode drag is in flight, else 0.0.
    drag_active: f32,
    // Pointer position (screen px) the dragged icon follows while dragging.
    drag_pos: vec2<f32>,
    // Open-transition values: (progress, opacity, start_scale, max_stagger).
    presentation: vec4<f32>,
    // Open-transition motion: (start_offset_fraction, _, _, _).
    presentation_motion: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) presentation_alpha: f32,
};

fn presentation_item_progress(center: vec2<f32>) -> f32 {
    let normalized = (center - u.frame_center) / max(u.frame_half_size, vec2<f32>(1.0));
    let radial = clamp(length(normalized), 0.0, 1.0);
    let delay = radial * u.presentation.w;
    let local = clamp((u.presentation.x - delay) / max(1.0 - delay, 0.001), 0.0, 1.0);
    return smoothstep(0.0, 1.0, local);
}

fn presentation_gather_offset(center: vec2<f32>, item_progress: f32) -> vec2<f32> {
    let normalized = (center - u.frame_center) / max(u.frame_half_size, vec2<f32>(1.0));
    let radial = length(normalized);
    var direction = vec2<f32>(0.0, 0.0);
    if radial > 0.001 {
        direction = normalized / radial;
    }
    let travel = min(u.frame_half_size.x, u.frame_half_size.y)
        * u.presentation_motion.x;
    // At progress 0 the item is outside its final position. It travels back
    // along the radial vector as the reveal progresses.
    return direction * travel * (1.0 - item_progress);
}

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, content px
    @location(1) uvrect: vec4<f32>, // (u0, v0, u1, v1)
    @location(2) color: vec4<f32>,  // non-premultiplied RGBA tint
    // NOTE: grid text has no clip region. The vertex buffer is the shared
    // 5-attribute / 80-byte GlyphQuad layout (which also carries clip_rect at
    // location 3 and clip_radius at location 4 for the control/modal text
    // shaders), but this grid-text entry point only consumes the first three
    // locations. Locations 3/4 are uploaded but ignored here.
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let c = corners[vi];

    let origin = vec2<f32>(xywh.x + u.scroll_x, xywh.y);
    let glyph_center = vec2<f32>(origin.x + xywh.z * 0.5, origin.y + xywh.w * 0.5);
    let item_progress = presentation_item_progress(glyph_center);
    let reveal_scale = mix(u.presentation.z, 1.0, item_progress);
    let gather_offset = presentation_gather_offset(glyph_center, item_progress);
    let world = glyph_center + gather_offset
        + (vec2<f32>(origin.x + c.x * xywh.z, origin.y + (1.0 - c.y) * xywh.w) - glyph_center)
            * reveal_scale;

    let half_vp = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half_vp.x) - 1.0,
        1.0 - (world.y / half_vp.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = vec2<f32>(
        mix(uvrect.x, uvrect.z, c.x),
        // c.y = 1 is the quad's top edge (glyph top) → map to atlas row v0
        // (the glyph's top row), which the CPU placed at atlas y = entry.y.
        mix(uvrect.w, uvrect.y, c.y),
    );
    out.color = color;
    out.presentation_alpha = u.presentation.y * item_progress;
    return out;
}

// Signed distance to a rounded box centered at the origin (shared with the
// tile/icon shaders).
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    // Clip labels to the fixed page frame so they never spill past its edge.
    let local = in.pos.xy - u.frame_center;
    let fd = sdRoundBox(local, u.frame_half_size, u.frame_radius);
    let frame_alpha = smoothstep(1.0, -1.0, fd);
    // Atlas stores RGBA; alpha is coverage. Color stays non-premultiplied for
    // the pipeline's standard alpha blending.
    return vec4<f32>(in.color.rgb, sampled.a * in.color.a * frame_alpha * in.presentation_alpha);
}
