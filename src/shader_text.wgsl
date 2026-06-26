// Text label shaders.
//
// One quad (two triangles) per glyph instance. The quad's screen position is
// offset by the same per-frame `scroll_x` as the tiles, so labels scroll in
// lockstep with icons. The fragment samples the glyph atlas and uses its
// alpha as coverage for each instance tint.

struct Uniforms {
    viewport: vec2<f32>,
    scroll_x: f32,
    _pad: f32,
    // Fixed page-frame center (physical px).
    frame_center: vec2<f32>,
    // Fixed page-frame half-size (physical px).
    frame_half_size: vec2<f32>,
    // Fixed page-frame corner radius (physical px) + pad.
    frame_radius: f32,
    frame_pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, content px
    @location(1) uvrect: vec4<f32>, // (u0, v0, u1, v1)
    @location(2) color: vec4<f32>,  // non-premultiplied RGBA tint
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
    let world = vec2<f32>(origin.x + c.x * xywh.z, origin.y + (1.0 - c.y) * xywh.w);

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
    return vec4<f32>(in.color.rgb, sampled.a * in.color.a * frame_alpha);
}
