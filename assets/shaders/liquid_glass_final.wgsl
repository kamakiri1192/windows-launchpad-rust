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

@group(0) @binding(0) var<uniform> u: GlassUniforms;
@group(0) @binding(1) var backdrop_texture: texture_2d<f32>;
@group(0) @binding(2) var backdrop_sampler: sampler;
@group(0) @binding(3) var geometry_texture: texture_2d<f32>;
@group(0) @binding(4) var blur_texture: texture_2d<f32>;

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

fn has_flag(bit: u32) -> bool {
    return (u.debug_flags & (1u << bit)) != 0u;
}

fn decode_displacement(encoded: vec2<f32>) -> vec2<f32> {
    return (encoded * 2.0 - vec2<f32>(1.0)) * max(u.max_displacement, 1.0);
}

fn sample_backdrop(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(backdrop_texture, backdrop_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}

fn sample_blurred_backdrop(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(blur_texture, backdrop_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}

fn sample_glass_backdrop(uv: vec2<f32>) -> vec4<f32> {
    if has_flag(7u) || u.blur_radius < 0.5 {
        return sample_backdrop(uv);
    }
    return sample_blurred_backdrop(uv);
}

fn apply_saturation(rgb: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luma = dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
    return mix(vec3<f32>(luma), rgb, saturation);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let frag_coord = in.position;
    let screen_uv = frag_coord.xy / max(u.viewport, vec2<f32>(1.0));
    let geometry_data = textureLoad(geometry_texture, vec2<i32>(frag_coord.xy), 0);

    if has_flag(0u) {
        let bg = sample_backdrop(screen_uv);
        return vec4<f32>(bg.rgb, 1.0);
    }
    if has_flag(1u) {
        return vec4<f32>(geometry_data.rgb, max(geometry_data.a, 0.35));
    }
    if has_flag(2u) {
        return vec4<f32>(geometry_data.rg, 0.5, max(geometry_data.a, 0.35));
    }
    if has_flag(3u) {
        return vec4<f32>(vec3<f32>(geometry_data.a), geometry_data.a);
    }

    if geometry_data.a < 0.01 {
        return vec4<f32>(0.0);
    }

    let displacement = decode_displacement(geometry_data.rg);
    let alpha = geometry_data.a;
    let normalized_height = geometry_data.b;

    var refract_color: vec4<f32>;
    if u.chromatic_aberration < 0.01 {
        let refracted_uv = screen_uv + displacement / u.viewport;
        refract_color = sample_glass_backdrop(refracted_uv);
    } else {
        let dispersion = u.chromatic_aberration * 0.5;
        let red_uv = screen_uv + displacement * (1.0 + dispersion) / u.viewport;
        let green_uv = screen_uv + displacement / u.viewport;
        let blue_uv = screen_uv + displacement * (1.0 - dispersion) / u.viewport;

        let red = sample_glass_backdrop(red_uv).r;
        let green_sample = sample_glass_backdrop(green_uv);
        let blue = sample_glass_backdrop(blue_uv).b;
        refract_color = vec4<f32>(red, green_sample.g, blue, green_sample.a);
    }

    let sharp_color = sample_backdrop(screen_uv + displacement * 0.35 / u.viewport);
    var final_rgb = mix(refract_color.rgb, sharp_color.rgb, 0.18);
    final_rgb = u.glass_color.rgb * u.glass_color.a
        + final_rgb * (1.0 - u.glass_color.a);
    final_rgb = apply_saturation(final_rgb, u.saturation);

    if !has_flag(6u) {
        let thickness_scale = clamp(40.0 / max(u.thickness, 1.0), 1.0, 4.0);
        let edge_threshold = mix(0.8, 0.5, 1.0 / thickness_scale);
        let edge_factor = 1.0 - smoothstep(0.0, edge_threshold, normalized_height);

        if edge_factor > 0.01 {
            let normal_xy = normalize(displacement + vec2<f32>(0.001, 0.001));
            let light_direction = normalize(u.light_direction + vec2<f32>(0.001, 0.0));
            let main_light = max(0.0, dot(normal_xy, light_direction));
            let opposite_light = max(0.0, dot(normal_xy, -light_direction));
            let total_influence = main_light + opposite_light * 0.8;
            let directional = pow(total_influence, 1.5) * u.light_intensity * 3.0;
            let ambient = u.ambient_strength * 0.5;
            let brightness = (directional + ambient) * edge_factor * thickness_scale * 0.8;

            let bg = sharp_color.rgb;
            let bg_luma = dot(bg, vec3<f32>(0.299, 0.587, 0.114));
            let saturated_bg = mix(bg, bg / max(bg_luma, 0.001), 0.8);
            let colorfulness = length(bg - vec3<f32>(bg_luma));
            let color_mix = clamp(colorfulness + 0.5, 0.5, 1.0);
            let highlight = mix(vec3<f32>(1.0), saturated_bg, color_mix);

            final_rgb = mix(final_rgb, highlight, clamp(brightness, 0.0, 1.0));
        }
    }

    if has_flag(4u) {
        return vec4<f32>(final_rgb * alpha, alpha);
    }

    return vec4<f32>(final_rgb * alpha, alpha);
}
