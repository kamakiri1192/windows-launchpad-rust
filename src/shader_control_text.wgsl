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
    @location(2) pixel_pos: vec2<f32>,
    @location(3) clip_rect: vec4<f32>,
    @location(4) clip_radius: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, physical px
    @location(1) uvrect: vec4<f32>, // (u0, v0, u1, v1)
    @location(2) color: vec4<f32>,  // non-premultiplied RGBA tint
    @location(3) clip_rect: vec4<f32>,  // (min_x, min_y, width, height); width<=0 → no clip
    @location(4) clip_radius: vec4<f32>, // (radius, 0, 0, 0)
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
    out.pixel_pos = world;
    out.clip_rect = clip_rect;
    out.clip_radius = clip_radius;
    return out;
}

// Signed distance to a rounded box centered at the origin.
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(b.x, b.y));
    let q = abs(p) - b + vec2<f32>(rr);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rr;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Per-instance clip: discard fragments outside the clip rect.
    if (in.clip_rect.z > 0.0) {
        let p = in.pixel_pos;
        let inside = p.x >= in.clip_rect.x && p.y >= in.clip_rect.y
                  && p.x < in.clip_rect.x + in.clip_rect.z
                  && p.y < in.clip_rect.y + in.clip_rect.w;
        if (!inside) {
            discard;
        }
        let r = in.clip_radius.x;
        if (r > 0.0) {
            let half = vec2<f32>(in.clip_rect.z, in.clip_rect.w) * 0.5;
            let center = vec2<f32>(in.clip_rect.x + half.x, in.clip_rect.y + half.y);
            let local_clip = p - center;
            let sd = sd_round_box(local_clip, half, r);
            if (sd > 0.0) {
                discard;
            }
        }
    }

    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    // Atlas stores RGBA; alpha is coverage. Color stays non-premultiplied.
    return vec4<f32>(in.color.rgb, sampled.a * in.color.a);
}
