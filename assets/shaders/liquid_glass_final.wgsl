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

fn safe_uv(uv: vec2<f32>) -> vec2<f32> {
    return clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
}

fn sample_backdrop(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(backdrop_texture, backdrop_sampler, safe_uv(uv));
}

fn sample_blurred_backdrop(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(blur_texture, backdrop_sampler, safe_uv(uv));
}

fn apply_saturation(rgb: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luma = dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
    return mix(vec3<f32>(luma), rgb, saturation);
}

fn luminance(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

fn colorfulness(rgb: vec3<f32>) -> f32 {
    let luma = luminance(rgb);
    return length(rgb - vec3<f32>(luma));
}

fn sample_geometry_data(pixel: vec2<f32>) -> vec4<f32> {
    let p = vec2<i32>(clamp(pixel, vec2<f32>(0.0), u.viewport - vec2<f32>(1.0)));
    return textureLoad(geometry_texture, p, 0);
}

fn sample_geometry_sd(pixel: vec2<f32>) -> f32 {
    return sample_geometry_data(pixel).a;
}

fn signed_distance_normal(pixel: vec2<f32>) -> vec2<f32> {
    let h_l = sample_geometry_sd(pixel + vec2<f32>(-1.0, 0.0));
    let h_r = sample_geometry_sd(pixel + vec2<f32>(1.0, 0.0));
    let h_u = sample_geometry_sd(pixel + vec2<f32>(0.0, -1.0));
    let h_d = sample_geometry_sd(pixel + vec2<f32>(0.0, 1.0));
    return normalize(vec2<f32>(h_r - h_l, h_d - h_u) + vec2<f32>(0.0001));
}

fn environmental_spill(screen_uv: vec2<f32>, inv_viewport: vec2<f32>) -> vec3<f32> {
    let r1 = 22.0 * inv_viewport;
    let r2 = 46.0 * inv_viewport;
    var acc = sample_backdrop(screen_uv).rgb * 1.8;
    acc += sample_backdrop(screen_uv + vec2<f32>(r1.x, 0.0)).rgb;
    acc += sample_backdrop(screen_uv + vec2<f32>(-r1.x, 0.0)).rgb;
    acc += sample_backdrop(screen_uv + vec2<f32>(0.0, r1.y)).rgb;
    acc += sample_backdrop(screen_uv + vec2<f32>(0.0, -r1.y)).rgb;
    acc += sample_backdrop(screen_uv + vec2<f32>(r2.x, r2.y)).rgb * 0.65;
    acc += sample_backdrop(screen_uv + vec2<f32>(-r2.x, r2.y)).rgb * 0.65;
    acc += sample_backdrop(screen_uv + vec2<f32>(r2.x, -r2.y)).rgb * 0.65;
    acc += sample_backdrop(screen_uv + vec2<f32>(-r2.x, -r2.y)).rgb * 0.65;
    return acc / 8.4;
}

fn shadow_color_and_alpha(
    sd: f32,
    pixel: vec2<f32>,
    screen_uv: vec2<f32>,
    inv_viewport: vec2<f32>,
) -> vec4<f32> {
    let shadow_margin = 72.0;
    let range = clamp(1.0 - smoothstep(0.0, shadow_margin, sd), 0.0, 1.0);
    let normal_xy = signed_distance_normal(pixel);
    let light_direction = normalize(u.light_direction + vec2<f32>(0.001, 0.0));
    let contact_side = smoothstep(-0.35, 0.85, dot(normal_xy, -light_direction));
    let contact = pow(range, 2.35) * (0.026 + 0.034 * contact_side) * u.light_intensity;

    let spill = environmental_spill(screen_uv, inv_viewport);
    let spill_luma = luminance(spill);
    let adaptive_strength = mix(1.16, 0.72, smoothstep(0.12, 0.82, spill_luma));
    let alpha = clamp(contact * adaptive_strength, 0.0, 0.09);
    let tint = mix(vec3<f32>(0.0), spill * 0.18, clamp(colorfulness(spill) * 1.1, 0.0, 0.16));
    return vec4<f32>(tint * alpha, alpha);
}

fn spill_color_and_alpha(
    sd: f32,
    screen_uv: vec2<f32>,
    inv_viewport: vec2<f32>,
) -> vec4<f32> {
    let spill = environmental_spill(screen_uv, inv_viewport);
    let edge = pow(clamp(1.0 - smoothstep(0.0, 26.0, sd), 0.0, 1.0), 3.0);
    let alpha = edge * (0.003 + colorfulness(spill) * 0.008);
    return vec4<f32>(spill * alpha, alpha);
}

fn compress_material_range(rgb: vec3<f32>, spill: vec3<f32>) -> vec3<f32> {
    let luma = luminance(rgb);
    let target_luma = mix(0.34, 0.72, smoothstep(0.12, 0.88, luminance(spill)));
    let leveled = rgb * (target_luma / max(luma, 0.08));
    return mix(rgb, leveled, 0.18);
}

fn fresnel_schlick(cos_theta: f32, ior: f32) -> f32 {
    let eta = max(ior, 1.001);
    let f0 = pow((eta - 1.0) / (eta + 1.0), 2.0);
    let grazing = pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0);
    return clamp(f0 + (1.0 - f0) * grazing, 0.0, 1.0);
}

fn beer_lambert(path_length: f32, spill: vec3<f32>) -> vec3<f32> {
    let cool_absorption = vec3<f32>(0.028, 0.018, 0.010);
    let warm_absorption = vec3<f32>(0.012, 0.017, 0.028);
    let absorption = mix(cool_absorption, warm_absorption, smoothstep(0.22, 0.78, luminance(spill)));
    return exp(-absorption * path_length);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let frag_coord = in.position;
    let pixel = frag_coord.xy;
    let screen_uv = pixel / max(u.viewport, vec2<f32>(1.0));
    let inv_viewport = vec2<f32>(1.0) / max(u.viewport, vec2<f32>(1.0));
    let geometry_data = textureLoad(geometry_texture, vec2<i32>(pixel), 0);
    let displacement = geometry_data.rg;
    let normalized_height = clamp(geometry_data.b, 0.0, 1.0);
    let sd = geometry_data.a;
    let shadow_margin = 72.0;

    if has_flag(0u) {
        let bg = sample_backdrop(screen_uv);
        return vec4<f32>(bg.rgb, 1.0);
    }
    if has_flag(1u) {
        let sd_vis = clamp(0.5 - sd / (shadow_margin * 2.0), 0.0, 1.0);
        return vec4<f32>(sd_vis, normalized_height, 1.0 - sd_vis, 1.0);
    }
    if has_flag(2u) {
        let max_d = max(u.max_displacement, 1.0);
        let disp = clamp(displacement / max_d * 0.5 + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
        return vec4<f32>(disp, 0.5, 1.0);
    }
    if has_flag(3u) {
        let alpha_mask = 1.0 - smoothstep(-1.5, 0.75, sd);
        return vec4<f32>(vec3<f32>(alpha_mask), alpha_mask);
    }

    if sd > shadow_margin {
        return vec4<f32>(0.0);
    }

    if sd >= 0.0 {
        let shadow = shadow_color_and_alpha(sd, pixel, screen_uv, inv_viewport);
        let spill = spill_color_and_alpha(sd, screen_uv, inv_viewport);
        if has_flag(8u) {
            return shadow;
        }
        if has_flag(9u) {
            return spill;
        }
        return shadow;
    }

    if has_flag(8u) || has_flag(9u) {
        return vec4<f32>(0.0);
    }

    let inside_alpha = 1.0 - smoothstep(-1.5, 0.75, sd);
    let edge_band = smoothstep(-34.0, -1.0, sd);
    let rim = pow(1.0 - smoothstep(0.04, 0.68, normalized_height), 1.08);
    let lens_weight = clamp(max(edge_band, rim), 0.0, 1.0);
    let lens_displacement = displacement * (0.86 + lens_weight * 1.15);
    let refract_uv = screen_uv + lens_displacement * inv_viewport;
    let normal_xy = signed_distance_normal(pixel);
    let view_cos = clamp(normalized_height * 0.84 + (1.0 - edge_band) * 0.16, 0.02, 1.0);
    let fresnel = fresnel_schlick(view_cos, u.refractive_index);

    var refract_rgb: vec3<f32>;
    if u.chromatic_aberration < 0.001 {
        refract_rgb = sample_backdrop(refract_uv).rgb;
    } else {
        let tangent = normalize(vec2<f32>(-lens_displacement.y, lens_displacement.x) + vec2<f32>(0.001, 0.0));
        let ca_px = (lens_displacement * u.chromatic_aberration * 0.55 + tangent * u.chromatic_aberration * 4.8) * rim;
        let red = sample_backdrop(refract_uv + ca_px * inv_viewport).r;
        let green = sample_backdrop(refract_uv).g;
        let blue = sample_backdrop(refract_uv - ca_px * inv_viewport).b;
        refract_rgb = vec3<f32>(red, green, blue);
    }

    let light_direction = normalize(u.light_direction + vec2<f32>(0.001, 0.0));
    let spill = environmental_spill(screen_uv, inv_viewport);
    let spill_luma = luminance(spill);
    let optical_path = clamp((0.45 + normalized_height * 1.35 + rim * 0.55) * u.thickness / 18.0, 0.0, 4.0);
    let transmittance = beer_lambert(optical_path, spill);
    let exit_uv = screen_uv + (lens_displacement - normal_xy * rim * 12.0) * inv_viewport;
    let exit_rgb = sample_backdrop(exit_uv).rgb;
    let transmitted = mix(refract_rgb, exit_rgb, 0.22) * transmittance;

    let reflection_uv = screen_uv - lens_displacement * 0.38 * inv_viewport
        + (normal_xy * 26.0 + light_direction * 18.0) * inv_viewport;
    let reflection_rgb = sample_backdrop(reflection_uv).rgb;
    let counter_reflect = sample_backdrop(screen_uv + normal_xy * 16.0 * inv_viewport - light_direction * 8.0 * inv_viewport).rgb;
    let reflection = mix(reflection_rgb, counter_reflect, edge_band * 0.28);
    var final_rgb = mix(transmitted, reflection, clamp(fresnel * (1.0 + rim * 2.2), 0.0, 0.72));

    if !has_flag(7u) && u.blur_radius >= 0.5 {
        let frost = sample_blurred_backdrop(screen_uv + lens_displacement * 0.06 * inv_viewport).rgb;
        let frost_mix = clamp(0.030 + u.blur_radius * 0.0018 + (1.0 - view_cos) * 0.045, 0.0, 0.105);
        final_rgb = mix(final_rgb, frost, frost_mix);
    }

    let adaptive_tint = mix(vec3<f32>(0.80, 0.88, 1.0), vec3<f32>(1.0, 0.98, 0.92), smoothstep(0.16, 0.86, spill_luma));
    final_rgb = compress_material_range(final_rgb, spill);
    final_rgb = mix(final_rgb, final_rgb * adaptive_tint + spill * 0.060, 0.16 + rim * 0.16);
    final_rgb = mix(final_rgb, u.glass_color.rgb, u.glass_color.a);
    final_rgb = apply_saturation(final_rgb, u.saturation);

    if !has_flag(6u) {
        let main_light = max(0.0, dot(-normal_xy, light_direction));
        let secondary_light = max(0.0, dot(normal_xy, light_direction));
        let grazing = pow(max(0.0, dot(normalize(lens_displacement + vec2<f32>(0.001)), light_direction)), 2.0);
        let rim_light = rim * (0.10 + 0.24 * main_light + 0.12 * grazing) * u.light_intensity;
        let highlight = mix(vec3<f32>(1.0), spill / max(spill_luma, 0.08), clamp(colorfulness(spill) * 1.25, 0.0, 0.45));
        final_rgb += highlight * rim_light * (0.45 + fresnel * 1.8);
        final_rgb += mix(spill, vec3<f32>(1.0), 0.62) * secondary_light * edge_band * 0.055 * u.light_intensity;

        let n3 = normalize(vec3<f32>(-normal_xy * 0.45, 0.82 + normalized_height * 0.18));
        let light3 = normalize(vec3<f32>(-light_direction, 0.86));
        let view3 = vec3<f32>(0.0, 0.0, 1.0);
        let specular = pow(max(dot(reflect(-light3, n3), view3), 0.0), 64.0)
            * rim
            * u.light_intensity
            * 0.24;
        final_rgb += mix(vec3<f32>(1.0), spill, 0.22) * specular;
    }

    final_rgb = clamp(final_rgb, vec3<f32>(0.0), vec3<f32>(1.25));
    let glass_alpha = clamp(inside_alpha * (0.34 + rim * 0.22 + fresnel * 0.24 + u.ambient_strength * 0.10), 0.0, 0.78);

    if has_flag(4u) {
        return vec4<f32>(final_rgb * glass_alpha, glass_alpha);
    }

    return vec4<f32>(final_rgb * glass_alpha, glass_alpha);
}
