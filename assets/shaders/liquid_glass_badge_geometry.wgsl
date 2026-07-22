// Sparse edit-badge geometry. Unlike the general Liquid Glass geometry pass,
// badges are small, isolated disks and never need a full-screen smooth union.
// One tightly bounded quad is rasterized per visible badge.

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
    time: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
    backdrop_origin: vec2<f32>,
    backdrop_extent: vec2<f32>,
};

struct GlassShape {
    center: vec2<f32>,
    size: vec2<f32>,
    radius: f32,
    shape_type: u32,
    motion: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: GlassUniforms;
@group(0) @binding(1) var<storage, read> shapes: array<GlassShape>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @interpolate(flat) @location(0) shape_index: u32,
};

fn resolved_center(shape: GlassShape) -> vec2<f32> {
    let t = u.time + shape.motion.z;
    let rot = sin(t * 8.0) * 0.06;
    let dy = abs(sin(t * 8.0)) * 2.0;
    let rel = shape.center - shape.motion.xy;
    let cosr = cos(rot);
    let sinr = sin(rot);
    return shape.motion.xy + vec2<f32>(
        rel.x * cosr - rel.y * sinr + u.scroll_x,
        rel.x * sinr + rel.y * cosr - dy,
    );
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) shape_index: u32,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, -1.0),
    );
    let shape = shapes[shape_index];
    let center = resolved_center(shape);
    let extent = shape.size * 0.5 + vec2<f32>(2.0);
    let corner = corners[vertex_index];
    let pixel = center + vec2<f32>(corner.x * extent.x, -corner.y * extent.y);
    let half_viewport = max(u.viewport * 0.5, vec2<f32>(0.5));

    var out: VsOut;
    out.position = vec4<f32>(
        pixel.x / half_viewport.x - 1.0,
        1.0 - pixel.y / half_viewport.y,
        0.0,
        1.0,
    );
    out.shape_index = shape_index;
    return out;
}

fn sdf_rrect(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(b.x, b.y));
    let q = abs(p) - b + vec2<f32>(rr);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rr;
}

fn encode_displacement(v: vec2<f32>) -> vec2<f32> {
    let max_d = max(u.max_displacement, 1.0);
    return clamp(v / max_d * 0.5 + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let pixel = in.position.xy;
    let shape = shapes[in.shape_index];
    let center = resolved_center(shape);
    let sd = sdf_rrect(pixel - center, shape.size * 0.5, shape.radius);

    // prepare_edit_badges always stores the fixed page clip at index zero.
    let clip = shapes[0u];
    let frame_sd = sdf_rrect(pixel - clip.center, clip.size * 0.5, clip.radius);
    let alpha = (1.0 - smoothstep(-2.0, 0.0, sd))
        * (1.0 - smoothstep(-2.0, 0.0, frame_sd));
    if alpha < 0.01 || sd >= 0.0 || u.thickness <= 0.0 {
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
    let incident = vec3<f32>(0.0, 0.0, -1.0);
    let refracted = refract(incident, normal, 1.0 / max(u.refractive_index, 1.001));
    let ray_len = (height + u.thickness * 8.0) / max(0.001, abs(refracted.z));
    let displacement = refracted.xy * ray_len;
    let normalized_height = clamp(height / max(u.thickness, 1.0), 0.0, 1.0);

    return vec4<f32>(encode_displacement(displacement), normalized_height, alpha);
}
