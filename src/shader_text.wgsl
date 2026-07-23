// Text label shaders (SDF drop-shadow + halo).
//
// One quad (two triangles) per glyph instance. The quad's screen position is
// offset by the same per-frame `scroll_x` as the tiles, so labels scroll in
// lockstep with icons. Mask glyphs are stored in the atlas as a signed
// distance field (see `text_engine.rs`); the fragment shader reconstructs the
// pixel distance and, from it, derives:
//
//   * the crisp glyph body (anti-aliased via smoothstep on the 0.5 isovalue),
//   * a Windows-10-style drop shadow offset toward the lower-right
//     (`1px 1px 2px rgba(0,0,0,.9)` in the CSS reference), and
//   * a soft all-directions halo (`0 0 4px rgba(0,0,0,.6)`).
//
// Colour (emoji) glyphs bypass the SDF path: they are stored as plain RGBA and
// drawn with straight alpha coverage.

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
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, content px
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
    out.extra = extra;
    return out;
}

// Signed distance to a rounded box centered at the origin (shared with the
// tile/icon shaders).
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// One device-independent pixel in physical units, for sizing the shadow
// offsets relative to the viewport height.
const PX: f32 = 1.0;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    // Clip labels to the fixed page frame so they never spill past its edge.
    let local = in.pos.xy - u.frame_center;
    let fd = sdRoundBox(local, u.frame_half_size, u.frame_radius);
    let frame_alpha = smoothstep(1.0, -1.0, fd);

    let is_sdf = in.extra.x >= 0.5;
    if (!is_sdf) {
        // Plain RGBA glyph (colour emoji): straight alpha, no shadow effects.
        return vec4<f32>(in.color.rgb * sampled.rgb, sampled.a * in.color.a * frame_alpha);
    }

    // --- SDF path ----------------------------------------------------------
    // Decode the distance field. The atlas stores the normalised distance in
    // the red channel: 0 = far outside (clamped at -spread), 0.5 = on the
    // outline, 1 = far inside (clamped at +spread). `dist_px` is the signed
    // physical-pixel distance to the glyph outline (>0 inside, <0 outside).
    let spread = max(in.extra.y, 0.0001);
    let dist_px = (sampled.r * 2.0 - 1.0) * spread;

    // Anti-aliasing half-width in px (~1px, screen-frequency dependent).
    let aa = max(fwidth(dist_px), 0.5);

    // Glyph body coverage (inside the outline). `dist_px > 0` inside, so a
    // positive distance maps to full coverage.
    let body = saturate(dist_px / aa + 0.5);

    // Main drop shadow: sample the field at a UV shifted toward the upper-left
    // so the outline it describes lands ~1 physical px toward the lower-right
    // of the current fragment (TextMeshPro "Underlay" trick). `fwidth(uv)` is
    // the per-pixel UV step, so PX * uv_step is a one-screen-pixel offset.
    let uv_step = fwidth(in.uv);
    let shadow_uv = in.uv - vec2<f32>(1.0, -1.0) * PX * uv_step;
    let shadow_sample = textureSample(atlas, atlas_sampler, shadow_uv);
    let shadow_dist = (shadow_sample.r * 2.0 - 1.0) * spread;
    // The shadow is the underlay's *body* (the shifted sample is inside the
    // outline), softened over ~2px to match the "1px 1px 2px" blur radius.
    let shadow_soft = max(fwidth(shadow_dist), 2.0);
    let shadow = saturate(shadow_dist / shadow_soft + 0.5);

    // Halo: a soft all-directions outline glow. Strongest right at the outline
    // (dist_px = 0) and fading out over `halo_radius` px into the outside.
    let halo_radius = spread;
    let halo = saturate(1.0 + dist_px / halo_radius);

    let body_alpha = body * in.color.a;
    let shadow_alpha = shadow * 0.9 * in.color.a;
    let halo_alpha = halo * 0.6 * in.color.a;

    // Composite: shadow and halo are black, drawn under the white body. The
    // body wins wherever it is opaque; otherwise the darker effects show.
    let black = max(shadow_alpha, halo_alpha);
    let out_a = max(black, body_alpha);
    let out_rgb = mix(vec3<f32>(0.0), in.color.rgb, body_alpha / max(out_a, 0.0001));
    return vec4<f32>(out_rgb, out_a * frame_alpha);
}
