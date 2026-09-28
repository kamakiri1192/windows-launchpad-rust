// Capture-wide diagnostic ONLY. Rendering evaluates the current pixel in the
// final fragment shader and never waits for this sampled/read-back value.
// Keep the uniform/state ABI and update modes for existing HUD consumers.
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

@group(0) @binding(0) var backdrop_texture: texture_2d<f32>;
@group(0) @binding(1) var backdrop_sampler: sampler;
@group(0) @binding(2) var<uniform> u: AutoDarknessUniforms;
@group(0) @binding(3) var<storage, read_write> state: AutoDarknessState;

var<workgroup> coverage_values: array<f32, 256>;
var<workgroup> probe_values: array<f32, 16>;
var<workgroup> run_full_classification: u32;

fn diagnostic_decode(v: f32) -> f32 {
    let c = max(v, 0.0);
    return select(c / 12.92, pow((c + 0.055) / 1.055, 2.4), c > 0.04045);
}

fn diagnostic_encode(v: f32) -> f32 {
    let c = max(v, 0.0);
    return select(c * 12.92, 1.055 * pow(c, 1.0 / 2.4) - 0.055, c > 0.0031308);
}

// Equivalent neutral-sRGB attenuation at the sample's luminance, normalized
// by the configured cap. It is NOT a scene-wide value applied to all glass.
fn diagnostic_score(rgb: vec3<f32>) -> f32 {
    let cap = clamp(u.max_darkness, 0.0, 1.0);
    let linear_rgb = vec3<f32>(diagnostic_decode(rgb.r),
        diagnostic_decode(rgb.g), diagnostic_decode(rgb.b));
    let y = dot(linear_rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    if cap <= 0.0 || y <= 0.0 {
        return 0.0;
    }
    if cap >= 1.0 {
        return 1.0;
    }
    let white = diagnostic_decode(1.0 - cap);
    let gain = white / (white + (1.0 - white) * clamp(y, 0.0, 1.0));
    return clamp((1.0 - diagnostic_encode(y * gain) / diagnostic_encode(y)) / cap, 0.0, 1.0);
}

// Legacy helper name retained for the diagnostic contract. There is no
// interpolation or history-dependent adaptation in the new color transform.
fn interpolated_darkness(_now: f32) -> f32 {
    return clamp(state.target_darkness, 0.0, clamp(u.max_darkness, 0.0, 1.0));
}

fn sample_grid(grid_size: u32, x: u32, y: u32) -> vec3<f32> {
    let uv = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5) / f32(grid_size);
    return textureSampleLevel(backdrop_texture, backdrop_sampler, uv, 0.0).rgb;
}

@compute @workgroup_size(16, 16, 1)
fn cs_main(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    if u.mode == 1u && index < 16u {
        probe_values[index] = diagnostic_score(sample_grid(4u, index % 4u, index / 4u));
    }
    workgroupBarrier();

    if index == 0u {
        let force = (u.flags & 1u) != 0u;
        let elapsed = u.now - state.last_full_update_at;
        var should_run = force || state.initialized == 0u;
        if u.mode == 0u {
            should_run = true;
        } else if u.mode == 2u {
            should_run = should_run || elapsed >= 0.125;
        } else {
            var probe_delta = 0.0;
            for (var i = 0u; i < 16u; i++) {
                probe_delta += abs(probe_values[i] - state.probe_samples[i]);
            }
            probe_delta /= 16.0;
            should_run = should_run || elapsed >= 0.125 || probe_delta >= 0.035;
        }
        run_full_classification = select(0u, 1u, should_run);
        // Also clamp a stale diagnostic when the maximum is lowered.
        state.displayed_darkness = interpolated_darkness(u.now);
    }
    workgroupBarrier();
    if run_full_classification == 0u {
        return;
    }

    coverage_values[index] = diagnostic_score(sample_grid(16u, local_id.x, local_id.y));
    workgroupBarrier();
    var stride = 128u;
    loop {
        if index < stride {
            coverage_values[index] += coverage_values[index + stride];
        }
        workgroupBarrier();
        if stride == 1u {
            break;
        }
        stride /= 2u;
    }

    if index == 0u {
        let mean_score = coverage_values[0] / 256.0;
        let next_target = mean_score * clamp(u.max_darkness, 0.0, 1.0);
        state.from_darkness = next_target;
        state.target_darkness = next_target;
        state.transition_started_at = u.now;
        state.filtered_coverage = mean_score;
        state.last_full_update_at = u.now;
        state.initialized = 1u;
        state.classification_count += 1u;
        state.displayed_darkness = interpolated_darkness(u.now);
        if u.mode == 1u {
            for (var i = 0u; i < 16u; i++) {
                state.probe_samples[i] = probe_values[i];
            }
        }
    }
}
