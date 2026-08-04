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
    // offset 0
    center: vec2<f32>,
    // offset 8
    size: vec2<f32>,
    // offset 16
    radius: f32,
    // offset 20
    shape_type: u32,
    // offset 24
    activation: f32,
    // offset 28 — explicit pad so clip_rect starts at 32 (16-byte aligned)
    _pad1: u32,
    // offset 32
    clip_rect: vec4<f32>,
    // offset 48
    clip_radius: f32,
    // offset 52 — explicit pad so motion starts at 64 (16-byte aligned)
    _pad2_a: u32,
    _pad2_b: u32,
    _pad2_c: u32,
    // offset 64
    motion: vec4<f32>,
    // offset 80; alpha < 0 uses the global glass tint
    tint: vec4<f32>,
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

/// Returns true when `pixel` falls inside the rounded rectangle defined by
/// `rect = (min_x, min_y, width, height)` with corner radius `r`.
/// When r <= 0.0, the check degrades to a plain axis-aligned rectangle test.
fn point_in_rounded_rect(pixel: vec2<f32>, rect: vec4<f32>, r: f32) -> bool {
    // Fast AABB reject: half-open on max edges, matching Rect::contains.
    if pixel.x < rect.x || pixel.y < rect.y
    || pixel.x >= rect.x + rect.z
    || pixel.y >= rect.y + rect.w {
        return false;
    }
    if r <= 0.0 {
        return true;
    }
    let half = vec2<f32>(rect.z, rect.w) * 0.5;
    let center = vec2<f32>(rect.x + half.x, rect.y + half.y);
    let sd = sdf_rrect(pixel - center, half, r);
    return sd <= 0.0;
}

fn smooth_union(d1: f32, d2: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return min(d1, d2);
    }
    let e = max(k - abs(d1 - d2), 0.0);
    return min(d1, d2) - e * e * 0.25 / k;
}

fn resolved_center(shape: GlassShape) -> vec2<f32> {
    var center = shape.center;
    if shape.shape_type == 4u || shape.shape_type == 5u || shape.shape_type == 6u {
        let t = u.time + shape.motion.z;
        let rot = sin(t * 8.0) * 0.06;
        let dy = abs(sin(t * 8.0)) * 2.0;
        let pivot = shape.motion.xy;
        let rel = center - pivot;
        let cosr = cos(rot);
        let sinr = sin(rot);
        center = pivot + vec2<f32>(
            rel.x * cosr - rel.y * sinr,
            rel.x * sinr + rel.y * cosr - dy,
        );
    }
    if shape.shape_type == 0u || shape.shape_type == 4u || shape.shape_type == 5u {
        center.x = center.x + u.scroll_x;
    }
    return center;
}

fn resolved_local(shape: GlassShape, pixel: vec2<f32>) -> vec2<f32> {
    let center = resolved_center(shape);
    var local = pixel - center;
    if shape.shape_type == 5u || shape.shape_type == 6u {
        // Rotate the sample point by the inverse parent wiggle so the rounded
        // folder rect and its miniature children behave as one rigid body.
        let t = u.time + shape.motion.z;
        let rot = -sin(t * 8.0) * 0.06;
        let cosr = cos(rot);
        let sinr = sin(rot);
        local = vec2<f32>(
            local.x * cosr - local.y * sinr,
            local.x * sinr + local.y * cosr,
        );
    }
    return local;
}

struct SceneSample {
    distance: f32,
    tint: vec4<f32>,
};

fn resolved_tint(shape: GlassShape) -> vec4<f32> {
    if shape.tint.a < 0.0 {
        return u.glass_color;
    }
    return clamp(shape.tint, vec4<f32>(0.0), vec4<f32>(1.0));
}

fn smooth_union_sample(a: SceneSample, b: SceneSample, k: f32) -> SceneSample {
    if k <= 0.0 {
        if a.distance <= b.distance {
            return a;
        }
        return b;
    }
    let h = clamp(0.5 + 0.5 * (b.distance - a.distance) / k, 0.0, 1.0);
    var result: SceneSample;
    result.distance = mix(b.distance, a.distance, h) - k * h * (1.0 - h);
    result.tint = mix(b.tint, a.tint, h);
    return result;
}

fn scene_sample(pixel: vec2<f32>) -> SceneSample {
    var result: SceneSample;
    result.distance = 1.0e6;
    result.tint = u.glass_color;
    var inside_tint = u.glass_color;
    var inside_distance = -1.0e6;
    var inside_found = false;
    let count = min(u.shape_count, arrayLength(&shapes));
    for (var i = 0u; i < count; i = i + 1u) {
        let shape = shapes[i];
        if shape.shape_type == 3u {
            continue;
        }
        // Per-shape clip: if clip_rect has positive width, only contribute
        // inside that rounded rectangle.
        if (shape.clip_rect.z > 0.0) {
            if (!point_in_rounded_rect(pixel, shape.clip_rect, shape.clip_radius)) {
                continue;
            }
        }
        let local = resolved_local(shape, pixel);
        let half_size = shape.size * 0.5;
        let shape_d = sdf_rrect(local, half_size, shape.radius);
        let shape_tint = resolved_tint(shape);
        var shape_sample: SceneSample;
        shape_sample.distance = shape_d;
        shape_sample.tint = shape_tint;
        result = smooth_union_sample(result, shape_sample, u.blend);

        // A nested surface can be geometrically swallowed by a larger smooth
        // union (for example a tinted child inside a panel). Prefer the
        // containing shape whose boundary is nearest to this pixel, while the
        // smooth-union tint remains the fallback in the bridge between shapes.
        if shape_d <= 0.0 && (!inside_found || shape_d >= inside_distance) {
            inside_tint = shape_tint;
            inside_distance = shape_d;
            inside_found = true;
        }
    }
    if inside_found {
        result.tint = inside_tint;
    }
    return result;
}

// Signed distance to the fixed page frame (the shape_type == 1 shape). Tiles'
// halos are clipped to this so they never spill past the frame while scrolling.
fn frame_sdf(pixel: vec2<f32>) -> f32 {
    let count = min(u.shape_count, arrayLength(&shapes));
    var d = 1.0e6;
    for (var i = 0u; i < count; i = i + 1u) {
        let shape = shapes[i];
        if shape.shape_type == 1u || shape.shape_type == 3u {
            // Clip the frame shape itself if it carries a per-shape clip.
            if (shape.clip_rect.z > 0.0) {
                if (!point_in_rounded_rect(pixel, shape.clip_rect, shape.clip_radius)) {
                    continue;
                }
            }
            let local = pixel - shape.center;
            d = sdf_rrect(local, shape.size * 0.5, shape.radius);
            return d;
        }
    }
    return d;
}

// Signed distance to frame-independent controls (shape_type == 2, or animated
// control == 6). These live outside the page frame and must NOT be clipped to
// it. Multiple control shapes are smooth-unioned so paired capsules can
// visibly attach and separate.
fn control_sdf(pixel: vec2<f32>) -> f32 {
    let count = min(u.shape_count, arrayLength(&shapes));
    var d = 1.0e6;
    for (var i = 0u; i < count; i = i + 1u) {
        let shape = shapes[i];
        if shape.shape_type == 2u || shape.shape_type == 6u {
            // Per-shape clip for control shapes.
            if (shape.clip_rect.z > 0.0) {
                if (!point_in_rounded_rect(pixel, shape.clip_rect, shape.clip_radius)) {
                    continue;
                }
            }
            let local = resolved_local(shape, pixel);
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

struct FsOut {
    @location(0) geometry: vec4<f32>,
    @location(1) tint: vec4<f32>,
};

fn empty_output() -> FsOut {
    var out: FsOut;
    out.geometry = vec4<f32>(0.0);
    out.tint = vec4<f32>(0.0);
    return out;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4<f32>) -> FsOut {
    let pixel = frag_coord.xy;
    let scene = scene_sample(pixel);
    let sd = scene.distance;
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
        return empty_output();
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
    var out: FsOut;
    out.geometry = vec4<f32>(encoded, normalized_height, clipped_alpha);
    out.tint = scene.tint;
    return out;
}
