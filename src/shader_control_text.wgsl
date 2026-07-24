// Bottom-control text shader.
//
// Draws glyph quads for the search pill label / field query / placeholder.
// Identical to the label text shader except it is NOT clipped to the fixed
// page frame (the control lives below the frame). Uses ALPHA_BLENDING so the
// per-glyph tint (which already carries the layer alpha) composites correctly.

struct Uniforms {
    viewport: vec2<f32>,
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
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, physical px
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

    // No scroll offset: control text is fixed on screen.
    let world = vec2<f32>(xywh.x + c.x * xywh.z, xywh.y + (1.0 - c.y) * xywh.w);

    let half_vp = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half_vp.x) - 1.0,
        1.0 - (world.y / half_vp.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = vec2<f32>(
        mix(uvrect.x, uvrect.z, c.x),
        mix(uvrect.w, uvrect.y, c.y),
    );
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    // Atlas stores RGBA; alpha is coverage. Color stays non-premultiplied.
    return vec4<f32>(in.color.rgb, sampled.a * in.color.a);
}

// Fixed-screen shadow-mask variant for control, modal, and settings text.
@fragment
fn fs_shadow(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    let coverage = sampled.a * in.color.a;
    return vec4<f32>(1.0, 0.0, 0.0, coverage);
}
