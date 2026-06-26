// Launchpad MVP shaders.
//
// Vertex shader draws a unit quad (4 verts, 6 indices) once per tile instance.
// The scroller's horizontal offset is applied here, per-frame, via a tiny
// uniform — so the instance buffer (tile positions/colors) is written only
// once at startup and never touched while scrolling.

struct Uniforms {
    // (viewport_w, viewport_h) in physical px.
    viewport: vec2<f32>,
    // Horizontal content offset (px). Negative scrolls right.
    scroll_x: f32,
    // Padding to keep the struct 16-byte aligned.
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

// Per-instance tile data.
struct InstanceIn {
    @location(0) origin: vec2<f32>,  // top-left in content px
    @location(1) size_r: vec2<f32>,  // (size, radius)
    @location(2) color: vec3<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,      // local coords in px, origin at center
    @location(1) size_r: vec2<f32>,
    @location(2) color: vec3<f32>,
};


// Unit quad: two triangles covering [0,1]x[0,1].
@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) origin: vec2<f32>,
    @location(1) size_r: vec2<f32>,
    @location(2) color: vec3<f32>,
) -> VsOut {
    // 0..1 unit quad corners.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let c = corners[vi];

    let size = size_r.x;
    // World-space top-left, shifted by the scroller.
    let tl = vec2<f32>(origin.x + u.scroll_x, origin.y);
    // Local pixel coordinates relative to the tile's center (for SDF).
    let local = vec2<f32>(c.x * size - size * 0.5, (1.0 - c.y) * size - size * 0.5);
    let world = vec2<f32>(tl.x + c.x * size, tl.y + (1.0 - c.y) * size);

    // Physical px → clip space. Y is flipped so that content origin is
    // top-left (matches our layout math).
    let half = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half.x) - 1.0,
        1.0 - (world.y / half.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = local;
    out.size_r = size_r;
    out.color = color;
    return out;
}

// Signed distance to a rounded box centered at the origin.
// `p` is in pixels relative to the tile center; `b` is half-extent.
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    let outer = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    return outer;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half_size = in.size_r.x * 0.5;
    let r = min(in.size_r.y, half_size);
    let d = sdRoundBox(in.uv, vec2<f32>(half_size, half_size), r);

    // 1px AA edge (assuming ~1px ≈ 1/dpiScale for now).
    let aa = 1.0;
    let alpha = smoothstep(aa, -aa, d);
    if alpha <= 0.001 {
        discard;
    }
    // Clip to the fixed page frame so tiles never spill past its rounded edge.
    let a = clip_to_frame(in.pos.xy, alpha);
    if a <= 0.001 {
        discard;
    }
    // Subtle top→bottom sheen for a touch of depth.
    let sheen = mix(1.08, 0.86, clamp(in.uv.y / in.size_r.x + 0.5, 0.0, 1.0));
    let col = in.color * sheen;
    return vec4<f32>(col, a);
}

// Clip `alpha` against the fixed page frame's rounded rect. `frag` is the
// fragment's physical-pixel position. Returns the (possibly zeroed) alpha.
fn clip_to_frame(frag: vec2<f32>, alpha: f32) -> f32 {
    let local = frag - u.frame_center;
    let fd = sdRoundBox(local, u.frame_half_size, u.frame_radius);
    // 1px AA edge around the frame border.
    let faa = 1.0;
    return min(alpha, smoothstep(faa, -faa, fd));
}
