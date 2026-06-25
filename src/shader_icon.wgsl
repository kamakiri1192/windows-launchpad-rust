// Icon pipeline shaders.
//
// One unit quad (two triangles) is drawn per icon instance. The instance
// carries the tile's geometry (so we can reuse the rounded-rect mask from the
// tile shader) and the UV rect into the shared icon atlas. The fragment
// samples the atlas, masks it to the rounded squircle, and composites with
// premultiplied alpha so icons blend correctly over the color tiles / glass.

struct Uniforms {
    // (viewport_w, viewport_h) in physical px.
    viewport: vec2<f32>,
    // Horizontal content offset (px). Negative scrolls right.
    scroll_x: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

// Per-instance icon data.
struct InstanceIn {
    @location(0) origin_size_r: vec4<f32>, // (x, y, size, radius)
    @location(1) uvrect: vec4<f32>,        // (u0, v0, u1, v1)
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Local coords in px, origin at tile center (for the SDF mask).
    @location(0) local: vec2<f32>,
    @location(1) size_r: vec2<f32>,
    @location(2) uv: vec2<f32>,
};

// Unit quad: two triangles covering [0,1]x[0,1].
@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) origin_size_r: vec4<f32>,
    @location(1) uvrect: vec4<f32>,
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

    let origin = origin_size_r.xy;
    let size = origin_size_r.z;
    let radius = origin_size_r.w;

    // World-space top-left, shifted by the scroller.
    let tl = vec2<f32>(origin.x + u.scroll_x, origin.y);
    // Local pixel coordinates relative to the tile's center (for the SDF).
    let local = vec2<f32>(c.x * size - size * 0.5, (1.0 - c.y) * size - size * 0.5);
    let world = vec2<f32>(tl.x + c.x * size, tl.y + (1.0 - c.y) * size);

    // Physical px → clip space. Y is flipped so content origin is top-left.
    let half = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half.x) - 1.0,
        1.0 - (world.y / half.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.local = local;
    out.size_r = vec2<f32>(size, radius);
    // Map quad corner → atlas UV. c.y=1 is the quad's top edge → v0.
    out.uv = vec2<f32>(
        mix(uvrect.x, uvrect.z, c.x),
        mix(uvrect.w, uvrect.y, c.y),
    );
    return out;
}

// Signed distance to a rounded box centered at the origin (same as tile shader).
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    let outer = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    return outer;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half_size = in.size_r.x * 0.5;
    let r = min(in.size_r.y, half_size);
    let d = sdRoundBox(in.local, vec2<f32>(half_size, half_size), r);

    // 1px AA edge on the squircle mask.
    let aa = 1.0;
    let mask = smoothstep(aa, -aa, d);
    if mask <= 0.001 {
        discard;
    }

    // Sample straight-alpha from the atlas, then premultiply for correct
    // blending (the pipeline uses PREMULTIPLIED_ALPHA_BLENDING over the tiles).
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    let rgb = sampled.rgb;
    let a = sampled.a * mask;
    return vec4<f32>(rgb * a, a);
}
