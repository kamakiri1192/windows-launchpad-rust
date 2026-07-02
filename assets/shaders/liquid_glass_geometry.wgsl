struct GlassUniforms {
    viewport: vec2<f32>,
    scroll_x: f32,
    thickness: f32,
    refractive_index: f32,
    chromatic_aberration: f32,
    blur_radius: f32,
    saturation: f32,
    glass_color: vec4<f32>,
    light_direction: vec2<f32>,
    light_intensity: f32,
    ambient_strength: f32,
    blend: f32,
    max_displacement: f32,
    shape_count: u32,
    debug_flags: u32,
};

struct GlassShape {
    center: vec2<f32>,
    size: vec2<f32>,
    radius: f32,
    shape_type: u32,
    pad: vec2<u32>,
};

@group(0) @binding(0) var<uniform> u: GlassUniforms;
@group(0) @binding(1) var<storage, read> shapes: array<GlassShape>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    let p = positions[vi];

    var out: VsOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + vec2<f32>(0.5);
    return out;
}

fn sdf_rrect(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let shortest = min(b.x, b.y);
    let rr = min(r, shortest);
    let q = abs(p) - b + vec2<f32>(rr);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rr;
}

fn smooth_union(d1: f32, d2: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return min(d1, d2);
    }
    let e = max(k - abs(d1 - d2), 0.0);
    return min(d1, d2) - e * e * 0.25 / k;
}

fn scene_sdf(pixel: vec2<f32>) -> f32 {
    var d = 1.0e6;
    let count = min(u.shape_count, arrayLength(&shapes));
    for (var i = 0u; i < count; i = i + 1u) {
        let shape = shapes[i];
        if shape.shape_type == 3u {
            continue;
        }
        // shape_type != 0 marks fixed shapes (the page frame = 1, the bottom
        // control = 2) that ignore scroll; only type 0 (tile halos) scrolls.
        let cx = select(shape.center.x + u.scroll_x, shape.center.x, shape.shape_type != 0u);
        let center = vec2<f32>(cx, shape.center.y);
        let local = pixel - center;
        let half_size = shape.size * 0.5;
        let shape_d = sdf_rrect(local, half_size, shape.radius);
        d = smooth_union(d, shape_d, u.blend);
    }
    return d;
}

// Signed distance to the fixed page frame (the shape_type == 1 shape). Tiles'
// halos are clipped to this so they never spill past the frame while scrolling.
fn frame_sdf(pixel: vec2<f32>) -> f32 {
    let count = min(u.shape_count, arrayLength(&shapes));
    var d = 1.0e6;
    for (var i = 0u; i < count; i = i + 1u) {
        let shape = shapes[i];
        if shape.shape_type == 1u || shape.shape_type == 3u {
            let local = pixel - shape.center;
            d = sdf_rrect(local, shape.size * 0.5, shape.radius);
            return d;
        }
    }
    return d;
}

// Signed distance to frame-independent controls (shape_type == 2). These live
// outside the page frame and must NOT be clipped to it. Multiple control shapes
// are smooth-unioned so paired capsules can visibly attach and separate.
fn control_sdf(pixel: vec2<f32>) -> f32 {
    let count = min(u.shape_count, arrayLength(&shapes));
    var d = 1.0e6;
    for (var i = 0u; i < count; i = i + 1u) {
        let shape = shapes[i];
        if shape.shape_type == 2u {
            let local = pixel - shape.center;
            let shape_d = sdf_rrect(local, shape.size * 0.5, shape.radius);
            d = smooth_union(d, shape_d, u.blend);
        }
    }
    return d;
}

fn encode_displacement(v: vec2<f32>) -> vec2<f32> {
    let max_d = max(u.max_displacement, 1.0);
    return clamp(v / max_d * 0.5 + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = frag_coord.xy;
    let sd = scene_sdf(pixel);
    let alpha = 1.0 - smoothstep(-2.0, 0.0, sd);

    // Clip the scrolling glass (frame + halos) to the fixed page frame so
    // scrolling halos never bleed past the frame's rounded edge.
    let fd = frame_sdf(pixel);
    let frame_clipped = alpha * (1.0 - smoothstep(-2.0, 0.0, fd));
    // The bottom control lives outside the frame; it is clipped only to its
    // own capsule, never to the frame.
    let cd = control_sdf(pixel);
    let control_alpha = alpha * (1.0 - smoothstep(-2.0, 0.0, cd));
    let clipped_alpha = max(frame_clipped, control_alpha);

    if clipped_alpha < 0.01 || sd >= 0.0 || u.thickness <= 0.0 {
        return vec4<f32>(0.0);
    }

    let dx = dpdx(sd);
    let dy = dpdy(sd);
    let n_cos = max(u.thickness + sd, 0.0) / u.thickness;
    let n_sin = sqrt(max(0.0, 1.0 - n_cos * n_cos));
    let normal = normalize(vec3<f32>(dx * n_cos, dy * n_cos, n_sin));

    let x = u.thickness + sd;
    let sqrt_term = sqrt(max(0.0, u.thickness * u.thickness - x * x));
    let height = select(sqrt_term, u.thickness, sd < -u.thickness);
    let base_height = u.thickness * 8.0;
    let incident = vec3<f32>(0.0, 0.0, -1.0);
    let inv_ri = 1.0 / max(u.refractive_index, 1.001);
    let refracted = refract(incident, normal, inv_ri);
    let ray_len = (height + base_height) / max(0.001, abs(refracted.z));
    let displacement = refracted.xy * ray_len;
    let normalized_height = clamp(height / max(u.thickness, 1.0), 0.0, 1.0);

    let encoded = encode_displacement(displacement);
    return vec4<f32>(encoded, normalized_height, clipped_alpha);
}
