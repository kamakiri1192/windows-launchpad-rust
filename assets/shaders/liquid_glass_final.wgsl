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
    // Per-lane glass activation (0.0 = standard glass, 1.0 = fully active).
    // Currently unused by this shader: the field exists to keep the uniform
    // layout in sync with the Rust `GlassUniforms` struct. Existing surfaces
    // pass activation = 0.0 and render exactly as before.
    activation: f32,
    // 0 = translucent glass over the existing target, 1 = replace the real
    // desktop with the captured-and-filtered backdrop inside the shape.
    backdrop_replacement: f32,
    auto_darkness_enabled: u32,
    backdrop_origin: vec2<f32>,
    backdrop_extent: vec2<f32>,
    glass_darkness: f32,
    presentation_alpha: f32,
    presentation_start_offset: f32,
    presentation_grid: u32,
    presentation: vec4<f32>,
    presentation_frame: vec4<f32>,
};

struct AutoDarknessUniforms {
    now: f32,
    mode: u32,
    flags: u32,
    max_darkness: f32,
};

struct AutoDarknessState {
    from_darkness: f32,
    target_darkness: f32,
    transition_started_at: f32,
    filtered_coverage: f32,
    last_full_update_at: f32,
    initialized: u32,
    classification_count: u32,
    displayed_darkness: f32,
    probe_samples: array<f32, 16>,
};

@group(0) @binding(0) var<uniform> u: GlassUniforms;
@group(0) @binding(1) var backdrop_texture: texture_2d<f32>;
@group(0) @binding(2) var backdrop_sampler: sampler;
@group(0) @binding(3) var geometry_texture: texture_2d<f32>;
@group(0) @binding(4) var blur_texture: texture_2d<f32>;
@group(0) @binding(5) var tint_texture: texture_2d<f32>;
@group(0) @binding(6) var<storage, read> auto_darkness: AutoDarknessState;
@group(0) @binding(7) var<uniform> auto_darkness_uniforms: AutoDarknessUniforms;

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

fn backdrop_uv(screen_uv: vec2<f32>) -> vec2<f32> {
    let screen_pixel = screen_uv * u.viewport;
    return (screen_pixel - u.backdrop_origin) / max(u.backdrop_extent, vec2<f32>(1.0));
}

fn sample_backdrop(screen_uv: vec2<f32>) -> vec4<f32> {
    let uv = backdrop_uv(screen_uv);
    return textureSample(backdrop_texture, backdrop_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}

fn sample_blurred_backdrop(screen_uv: vec2<f32>) -> vec4<f32> {
    let uv = backdrop_uv(screen_uv);
    return textureSample(blur_texture, backdrop_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}

fn sample_glass_backdrop(uv: vec2<f32>) -> vec4<f32> {
    if has_flag(7u) || u.blur_radius < 0.5 {
        return sample_backdrop(uv);
    }
    // Every lane binds a fully reconstructed blur output. Pyramid levels are
    // workspaces and must never be sampled here as blur-strength profiles.
    return sample_blurred_backdrop(uv);
}

fn apply_saturation(rgb: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luma = dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
    return mix(vec3<f32>(luma), rgb, saturation);
}

// Capture-wide diagnostic accessor retained for tooling/ABI compatibility.
// Not used by the material; this is NOT its spatially varying correction.
fn auto_glass_darkness() -> f32 {
    if u.auto_darkness_enabled == 0u {
        return 0.0;
    }

    if auto_darkness.initialized == 0u {
        return 0.0;
    }

    return auto_darkness.displayed_darkness;
}

// BEGIN PERCEPTUAL_DARKNESS (pure functions; exercised by the QA harness)
// The backdrop color handled here follows the existing encoded-sRGB domain.
// Decode it once for this SDR adjustment, then return to that same domain.
// Render-target/OS color management and the rest of the renderer are unchanged.
fn darkness_srgb_to_linear(rgb: vec3<f32>) -> vec3<f32> {
    let c = max(rgb, vec3<f32>(0.0));
    return select(c / 12.92, pow((c + 0.055) / 1.055, vec3<f32>(2.4)),
        c > vec3<f32>(0.04045));
}

fn darkness_linear_to_srgb(rgb: vec3<f32>) -> vec3<f32> {
    let c = max(rgb, vec3<f32>(0.0));
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055,
        c > vec3<f32>(0.0031308));
}

fn darkness_luminance(linear_rgb: vec3<f32>) -> f32 {
    return dot(linear_rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Bounded, monotone Reinhard-type shoulder, calibrated so white maps to
// sRGB (1 - maximum). Multiplying LINEAR RGB by one gain preserves chromaticity.
// No histogram, chroma veto, scene-wide exposure, or previous-frame state.
fn darkness_auto_gain(y: f32, maximum: f32) -> f32 {
    let cap = clamp(maximum, 0.0, 1.0);
    if cap <= 0.0 {
        return 1.0;
    }
    if cap >= 1.0 {
        return 0.0;
    }
    let white = darkness_srgb_to_linear(vec3<f32>(1.0 - cap)).r;
    return white / (white + (1.0 - white) * clamp(y, 0.0, 1.0));
}

// Manual darkness keeps its legacy encoded-RGB meaning. Auto supplies a
// luminance ceiling, not an additional black overlay: when manual already
// reaches that ceiling, return the manual result unchanged. The crossing is
// continuous even for saturated colors, unlike switching between two RGB maps.
fn perceptual_darkness_color(rgb: vec3<f32>, manual: f32, maximum: f32) -> vec3<f32> {
    let m = clamp(manual, 0.0, 1.0);
    let manual_rgb = rgb * (1.0 - m);
    if maximum <= 0.0 || m >= 1.0 {
        return manual_rgb;
    }
    let linear_rgb = darkness_srgb_to_linear(rgb);
    let y = darkness_luminance(linear_rgb);
    if y <= 0.0 {
        return manual_rgb;
    }
    let target_y = y * darkness_auto_gain(y, maximum);
    var manual_linear = linear_rgb;
    if m > 0.0 {
        manual_linear = darkness_srgb_to_linear(manual_rgb);
    }
    let manual_y = darkness_luminance(manual_linear);
    if manual_y <= target_y {
        return manual_rgb;
    }
    let gain = target_y / manual_y;
    let output_linear = manual_linear * gain;
    // The rounded sRGB breakpoints have a tiny overlap. Preserve the exact
    // piecewise definition there instead of assuming its two branches join.
    if (manual_rgb.r <= 0.04045 && output_linear.r > 0.0031308)
        || (manual_rgb.g <= 0.04045 && output_linear.g > 0.0031308)
        || (manual_rgb.b <= 0.04045 && output_linear.b > 0.0031308) {
        return darkness_linear_to_srgb(output_linear);
    }
    // Exact sRGB attenuation with ONE scalar encode power, rather than
    // three component powers: for the nonlinear branch, E(g*D(c)) equals
    // (c+0.055)*pow(g,1/2.4)-0.055. A component crossing the toe uses the
    // linear branch instead. No gamma approximation or color LUT is used.
    let nonlinear = (max(manual_rgb, vec3<f32>(0.0)) + 0.055)
        * pow(gain, 1.0 / 2.4) - 0.055;
    return select(output_linear * 12.92, nonlinear,
        output_linear > vec3<f32>(0.0031308));
}
// END PERCEPTUAL_DARKNESS

fn apply_glass_darkness(rgb: vec3<f32>) -> vec3<f32> {
    // The same filtered pixel being rendered drives adaptation. A stale HUD
    // readback, an 8 Hz diagnostic mode, and even an uninitialized stats buffer
    // cannot delay this path. Preserve manual-only output when auto is off.
    let maximum = select(0.0, auto_darkness_uniforms.max_darkness,
        u.auto_darkness_enabled != 0u);
    return perceptual_darkness_color(rgb, u.glass_darkness, maximum);
}

fn sample_geometry_height(pixel: vec2<f32>) -> f32 {
    let p = vec2<i32>(clamp(pixel, vec2<f32>(0.0), u.viewport - vec2<f32>(1.0)));
    return textureLoad(geometry_texture, p, 0).b;
}

fn luminance(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

// Settings thumb only: binding 1 is the sharp, completed in-app scene (including
// the green track), and binding 4 is the desktop for any transparent pixels.
// Do not reuse the panel material's blur, adaptive tint, or darkness here.
@fragment
fn fs_toggle(in: VsOut) -> @location(0) vec4<f32> {
    let geometry = textureLoad(geometry_texture, vec2<i32>(in.position.xy), 0);
    if geometry.a < 0.01 {
        return vec4<f32>(0.0);
    }
    let height = clamp(geometry.b, 0.0, 1.0);
    let rim = sqrt(max(1.0 - height * height, 0.0));
    let displacement = decode_displacement(geometry.rg);
    // Panel optics can displace hundreds of pixels. Keep this small lens local
    // to the track, with a clear, undistorted interior and only a refracting rim.
    let offset = displacement / max(length(displacement), 1.0)
        * min(length(displacement), 2.5) * rim;
    let uv = clamp((in.position.xy + offset) / u.viewport,
        vec2<f32>(0.0), vec2<f32>(1.0));
    let scene = textureSampleLevel(backdrop_texture, backdrop_sampler, uv, 0.0);
    let desktop = textureSampleLevel(blur_texture, backdrop_sampler,
        clamp(backdrop_uv(uv), vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    var rgb = scene.rgb + desktop.rgb * (1.0 - scene.a);
    let normal = normalize(displacement + vec2<f32>(0.001));
    let light = max(dot(normal, normalize(vec2<f32>(-0.6, -0.8))), 0.0);
    // A narrow light/dark rim keeps the deforming silhouette readable without
    // laying a white film across the clear interior.
    rgb *= 1.0 - rim * 0.20 * (1.0 - light);
    rgb = mix(rgb, vec3<f32>(1.0), rim * light * 0.30);
    return vec4<f32>(rgb * geometry.a, geometry.a);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let frag_coord = in.position;
    let screen_uv = frag_coord.xy / max(u.viewport, vec2<f32>(1.0));
    let pixel = vec2<i32>(frag_coord.xy);
    let geometry_data = textureLoad(geometry_texture, pixel, 0);
    let glass_tint = textureLoad(tint_texture, pixel, 0);

    if has_flag(0u) {
        let bg = sample_backdrop(screen_uv);
        return vec4<f32>(bg.rgb * u.presentation_alpha, u.presentation_alpha);
    }
    if has_flag(1u) {
        return vec4<f32>(geometry_data.rgb * u.presentation_alpha, max(geometry_data.a, 0.35) * u.presentation_alpha);
    }
    if has_flag(2u) {
        return vec4<f32>(geometry_data.rg * u.presentation_alpha, 0.5 * u.presentation_alpha, max(geometry_data.a, 0.35) * u.presentation_alpha);
    }
    if has_flag(3u) {
        return vec4<f32>(vec3<f32>(geometry_data.a) * u.presentation_alpha, geometry_data.a * u.presentation_alpha);
    }

    if geometry_data.a < 0.01 {
        return vec4<f32>(0.0);
    }

    let displacement = decode_displacement(geometry_data.rg);
    let alpha = geometry_data.a;
    let normalized_height = geometry_data.b;
    let inv_viewport = vec2<f32>(1.0) / max(u.viewport, vec2<f32>(1.0));

    // More than one thickness inside a glass boundary the encoded height is
    // exactly 1, displacement is zero, and every rim/reflection term below is
    // zero. Preserve the same color equation with one blurred and one sharp
    // sample instead of paying for chromatic separation, reflection, normal
    // reconstruction, and caustics across the large flat interior.
    if normalized_height >= 1.0 {
        let filtered_color = sample_glass_backdrop(screen_uv);
        let sharp_color = sample_backdrop(screen_uv);
        let replacement = clamp(u.backdrop_replacement, 0.0, 1.0);
        // Ordinary glass retains a little crispness. A backdrop-replacing
        // material must not reintroduce the sharp capture before composition.
        let sharp_interior_mix = 0.12 * (1.0 - replacement);
        var interior_rgb = mix(filtered_color.rgb, sharp_color.rgb, sharp_interior_mix);
        let bg_luma = luminance(interior_rgb);
        let adaptive_tint = mix(vec3<f32>(0.82, 0.90, 1.0), vec3<f32>(1.0, 0.98, 0.94), smoothstep(0.15, 0.85, bg_luma));
        interior_rgb = mix(interior_rgb, interior_rgb * adaptive_tint + adaptive_tint * 0.045, 0.55);
        interior_rgb = glass_tint.rgb * glass_tint.a
            + interior_rgb * (1.0 - glass_tint.a);
        interior_rgb = apply_saturation(interior_rgb, u.saturation);
        interior_rgb = apply_glass_darkness(interior_rgb);
        interior_rgb = clamp(interior_rgb, vec3<f32>(0.0), vec3<f32>(1.45));
        let translucent_alpha = clamp(alpha * (0.64 + glass_tint.a * 0.5), 0.0, 0.92);
        // At replacement=1 the lane's filtered backdrop becomes the actual
        // pixel under the surface, so DirectComposition cannot blend the sharp
        // desktop back through it. Context-menu lanes bind a completed scene;
        // ordinary lanes bind the captured desktop. Geometry alpha still
        // provides antialiased edge coverage.
        let interior_alpha = mix(translucent_alpha, alpha, replacement);
        return vec4<f32>(interior_rgb * interior_alpha * u.presentation_alpha, interior_alpha * u.presentation_alpha);
    }

    let refract_uv = screen_uv + displacement * inv_viewport;
    // Per-surface activation (0 = standard glass, 1 = enhanced): a wider rim
    // makes an interactive lens (e.g. a toggle thumb) read as "pressed".
    let edge_power = mix(1.75, 1.15, u.activation);
    let edge_factor = pow(1.0 - clamp(normalized_height, 0.0, 1.0), edge_power);

    var refract_color: vec4<f32>;
    let effective_chromatic = u.chromatic_aberration * (1.0 + u.activation * 0.6);
    if effective_chromatic < 0.01 {
        refract_color = sample_glass_backdrop(refract_uv);
    } else {
        let dispersion = effective_chromatic * (0.45 + edge_factor * 1.7);
        let tangent = normalize(vec2<f32>(-displacement.y, displacement.x) + vec2<f32>(0.001, 0.0));
        let prism = tangent * edge_factor * 3.0;
        let red_uv = screen_uv + (displacement * (1.0 + dispersion) + prism) * inv_viewport;
        let green_uv = refract_uv;
        let blue_uv = screen_uv + (displacement * (1.0 - dispersion) - prism) * inv_viewport;

        let red = sample_glass_backdrop(red_uv).r;
        let green_sample = sample_glass_backdrop(green_uv);
        let blue = sample_glass_backdrop(blue_uv).b;
        refract_color = vec4<f32>(red, green_sample.g, blue, green_sample.a);
    }

    let sharp_color = sample_backdrop(screen_uv + displacement * 0.28 * inv_viewport);
    let reflection_color = sample_backdrop(screen_uv - displacement * 0.42 * inv_viewport + normalize(u.light_direction) * 0.035);
    let replacement = clamp(u.backdrop_replacement, 0.0, 1.0);
    let sharp_edge_mix = 0.12 * (1.0 - replacement);
    var final_rgb = mix(refract_color.rgb, sharp_color.rgb, sharp_edge_mix);
    // Stronger reflection pull on the rim when the lens is active.
    let reflection_mix = mix(0.22 + 0.18 * u.activation, 0.08, replacement);
    final_rgb = mix(final_rgb, reflection_color.rgb, edge_factor * reflection_mix);

    let bg_luma = luminance(final_rgb);
    let adaptive_tint = mix(vec3<f32>(0.82, 0.90, 1.0), vec3<f32>(1.0, 0.98, 0.94), smoothstep(0.15, 0.85, bg_luma));
    final_rgb = mix(final_rgb, final_rgb * adaptive_tint + adaptive_tint * 0.045, 0.55);
    final_rgb = glass_tint.rgb * glass_tint.a
        + final_rgb * (1.0 - glass_tint.a);
    // Saturation stays at the configured value at activation = 0 and pushes a
    // touch further only when active (never *reduces* it below standard).
    let effective_saturation = u.saturation + 0.12 * u.activation;
    final_rgb = apply_saturation(final_rgb, effective_saturation);
    final_rgb = apply_glass_darkness(final_rgb);

    if !has_flag(6u) {
        let thickness_scale = clamp(40.0 / max(u.thickness, 1.0), 1.0, 4.0);
        let edge_threshold = mix(0.8, 0.5, 1.0 / thickness_scale);
        let rim = 1.0 - smoothstep(0.0, edge_threshold, normalized_height);

        if rim > 0.01 {
            let h_l = sample_geometry_height(frag_coord.xy + vec2<f32>(-1.0, 0.0));
            let h_r = sample_geometry_height(frag_coord.xy + vec2<f32>(1.0, 0.0));
            let h_u = sample_geometry_height(frag_coord.xy + vec2<f32>(0.0, -1.0));
            let h_d = sample_geometry_height(frag_coord.xy + vec2<f32>(0.0, 1.0));
            let height_gradient = vec2<f32>(h_r - h_l, h_d - h_u);
            let normal_xy = normalize(displacement + vec2<f32>(0.001, 0.001));
            let light_direction = normalize(u.light_direction + vec2<f32>(0.001, 0.0));
            let main_light = max(0.0, dot(normal_xy, light_direction));
            let opposite_light = max(0.0, dot(normal_xy, -light_direction));
            let total_influence = main_light + opposite_light * 0.8;
            let directional = pow(total_influence, 1.5) * u.light_intensity * 3.0;
            let ambient = u.ambient_strength * 0.5;
            // Edge lighting brightens with activation but keeps its standard
            // strength at activation = 0.
            let brightness = (directional + ambient) * rim * thickness_scale * 0.8 * (1.0 + 0.6 * u.activation);

            let bg = sharp_color.rgb;
            let bg_luma = luminance(bg);
            let saturated_bg = mix(bg, bg / max(bg_luma, 0.001), 0.8);
            let colorfulness = length(bg - vec3<f32>(bg_luma));
            let color_mix = clamp(colorfulness + 0.5, 0.5, 1.0);
            let highlight = mix(vec3<f32>(1.0), saturated_bg, color_mix);

            final_rgb = mix(final_rgb, highlight, clamp(brightness, 0.0, 1.0));

            let n3 = normalize(vec3<f32>(normal_xy * 0.72 + height_gradient * 8.0, 0.58 + normalized_height * 0.42));
            let light3 = normalize(vec3<f32>(-light_direction, 0.82));
            let view3 = vec3<f32>(0.0, 0.0, 1.0);
            let specular = pow(max(dot(reflect(-light3, n3), view3), 0.0), 42.0)
                * u.light_intensity
                * (0.25 + rim * 1.65)
                * (1.0 + u.activation);
            let caustic_phase = sin((screen_uv.x * 19.0 + screen_uv.y * 13.0 + u.scroll_x * 0.012) * 6.28318);
            let caustic = pow(clamp(length(height_gradient) * 18.0 + rim * 0.45, 0.0, 1.0), 2.0)
                * (0.55 + 0.45 * caustic_phase)
                * u.light_intensity
                * (1.0 + 0.5 * u.activation);
            final_rgb += vec3<f32>(1.0, 0.96, 0.88) * specular;
            final_rgb += mix(vec3<f32>(0.25, 0.55, 1.0), vec3<f32>(1.0, 0.92, 0.55), main_light) * caustic * 0.18;
        }
    }

    final_rgb = clamp(final_rgb, vec3<f32>(0.0), vec3<f32>(1.45));
    let translucent_alpha = clamp(alpha * (0.64 + edge_factor * 0.26 + glass_tint.a * 0.5), 0.0, 0.92);
    let glass_alpha = mix(translucent_alpha, alpha, replacement);

    if has_flag(4u) {
        return vec4<f32>(final_rgb * glass_alpha * u.presentation_alpha, glass_alpha * u.presentation_alpha);
    }

    return vec4<f32>(final_rgb * glass_alpha * u.presentation_alpha, glass_alpha * u.presentation_alpha);
}
