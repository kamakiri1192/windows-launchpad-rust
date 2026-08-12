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

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

fn chroma(rgb: vec3<f32>) -> f32 {
    return max(rgb.r, max(rgb.g, rgb.b)) - min(rgb.r, min(rgb.g, rgb.b));
}

fn white_score(rgb: vec3<f32>) -> f32 {
    let bright = smoothstep(0.68, 0.90, luma(rgb));
    let neutral = 1.0 - smoothstep(0.10, 0.26, chroma(rgb));
    return bright * neutral;
}

fn interpolated_darkness(now: f32) -> f32 {
    let duration = select(0.30, 0.18, state.target_darkness > state.from_darkness);
    let t = clamp((now - state.transition_started_at) / duration, 0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    return mix(state.from_darkness, state.target_darkness, eased);
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
    // Adaptive mode runs a 4x4 probe for every new capture. Full 16x16
    // classification wakes immediately when the probe changes, remains at
    // full rate near the decision boundary, and otherwise settles to 8 Hz.
    if u.mode == 1u && index < 16u {
        let probe_x = index % 4u;
        let probe_y = index / 4u;
        probe_values[index] = white_score(sample_grid(4u, probe_x, probe_y));
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
    }
    workgroupBarrier();

    // Keep transition evaluation at the capture/display cadence even when a
    // stable backdrop skips the expensive 16x16 classification. The final
    // fragment pass then reads one scalar instead of repeating this math for
    // every covered pixel.
    if index == 0u {
        state.displayed_darkness = interpolated_darkness(u.now);
    }

    if run_full_classification == 0u {
        return;
    }

    let rgb = sample_grid(16u, local_id.x, local_id.y);
    coverage_values[index] = white_score(rgb);
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
        let coverage = coverage_values[0] / 256.0;
        let elapsed = max(u.now - state.last_full_update_at, 0.0);
        let filter_alpha = select(
            1.0 - exp(-elapsed / 0.12),
            1.0,
            state.initialized == 0u,
        );
        let filtered = mix(state.filtered_coverage, coverage, filter_alpha);
        let next_target = smoothstep(0.55, 0.82, filtered) * clamp(u.max_darkness, 0.0, 1.0);

        if state.initialized == 0u {
            state.from_darkness = 0.0;
            state.target_darkness = next_target;
            state.transition_started_at = u.now;
            state.initialized = 1u;
        } else if abs(next_target - state.target_darkness) >= 0.001 {
            state.from_darkness = state.displayed_darkness;
            state.target_darkness = next_target;
            state.transition_started_at = u.now;
        }

        state.filtered_coverage = filtered;
        state.last_full_update_at = u.now;
        state.classification_count += 1u;
        if u.mode == 1u {
            for (var i = 0u; i < 16u; i++) {
                state.probe_samples[i] = probe_values[i];
            }
        }
    }
}
