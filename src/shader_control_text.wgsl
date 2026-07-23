// Bottom-control / modal / settings text shader.
//
// Same glyph-quad pipeline as `shader_text.wgsl` but without the fixed
// page-frame clip (controls/modals live outside or above the page frame).
// Shares the SDF drop-shadow + halo effect from the label shader so folder
// titles, settings text, and search-field text all stay legible on the moving
// blurred scene. Colour (emoji) glyphs bypass the SDF path.

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
    // (is_sdf, spread, 0, 0) — passed through for the fragment branch.
    @location(2) extra: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, physical px
    @location(1) uvrect: vec4<f32>, // (u0, v0, u1, v1)
    @location(2) color: vec4<f32>,  // non-premultiplied RGBA tint
    @location(3) extra: vec4<f32>,  // (is_sdf, spread, ..)
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
    out.extra = extra;
    return out;
}

// One device-independent pixel in physical units.
const PX: f32 = 1.0;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas, atlas_sampler, in.uv);

    let is_sdf = in.extra.x >= 0.5;
    if (!is_sdf) {
        // Plain RGBA glyph (colour emoji): straight alpha, no shadow effects.
        return vec4<f32>(in.color.rgb * sampled.rgb, sampled.a * in.color.a);
    }

    // --- SDF path ----------------------------------------------------------
    // See shader_text.wgsl for the full rationale. Same Windows-10-style
    // drop shadow + halo, minus the page-frame clip.
    let spread = max(in.extra.y, 0.0001);
    let dist_px = (sampled.r * 2.0 - 1.0) * spread;
    let aa = max(fwidth(dist_px), 0.5);
    let body = saturate((0.0 - dist_px) / aa + 0.5);

    let uv_step = fwidth(in.uv);
    let shadow_uv = in.uv - vec2<f32>(1.0, -1.0) * PX * uv_step;
    let shadow_sample = textureSample(atlas, atlas_sampler, shadow_uv);
    let shadow_dist = (shadow_sample.r * 2.0 - 1.0) * spread;
    let shadow_soft = max(fwidth(shadow_dist), 2.0);
    let shadow = saturate((shadow_soft - shadow_dist) / (2.0 * shadow_soft));

    let halo_radius = 4.0;
    let halo = saturate((halo_radius - abs(dist_px)) / halo_radius);

    let body_alpha = body * in.color.a;
    let shadow_alpha = shadow * 0.9 * in.color.a;
    let halo_alpha = halo * 0.6 * in.color.a;

    let black = max(shadow_alpha, halo_alpha);
    let out_a = max(black, body_alpha);
    let out_rgb = mix(vec3<f32>(0.0), in.color.rgb, body_alpha / max(out_a, 0.0001));
    return vec4<f32>(out_rgb, out_a);
}
