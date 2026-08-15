// Automatic-darkness backdrop classification (issue #186).
//
// One 16x16 workgroup samples the backdrop, derives a single scalar
// "target darkness", and keeps it in `state` where the final glass pass
// interpolates it per frame. The scalar is produced by one of three
// strategies selected with `u.strategy`:
//
//   strategy 0 (Current): PR #184 white-coverage heuristic, unchanged
//     baseline. bright = smoothstep(0.68, 0.90, luma), neutral =
//     1 - smoothstep(0.10, 0.26, chroma); coverage = mean(bright*neutral);
//     target = smoothstep(0.55, 0.82, filtered_coverage) * max_darkness.
//
//   strategy 1 (CoverageTuned): identical pipeline, but every tunable
//     constant is lifted into the named consts below (TUNED_*) so the
//     calibration harness can sweep them. Values are the harness winners
//     (docs/qa/auto-darkness/results.md).
//
//   strategy 2 (ContrastPerceptual): sRGB-decode each sample to relative
//     luminance, build an 8-bin histogram over [0, 1], take the QUANTILE
//     quantile (0.75 * 256 = 192 samples), and invert that luminance
//     into a target darkness so that the glass keeps an approximately
//     constant target luminance L_TARGET under the backdrop:
//     target = clamp(1 - L_TARGET / quantile_luma, 0, max_darkness).
//     Dark backdrops (quantile < L_TARGET) stay clear; bright backdrops
//     push toward max_darkness.
//
// The EMA smoothing (0.12 s filter constant), transition easing, hysteresis
// and the 8 Hz throttling are shared by all three strategies.

struct AutoDarknessUniforms {
    now: f32,
    mode: u32,
    flags: u32,
    max_darkness: f32,
    // 0 = PR #184 white-coverage baseline, 1 = recalibrated coverage,
    // 2 = perceptual-luminance contrast inversion. See issue #186.
    strategy: u32,
    // Local-region judgment (戦略D): when 1, the 16x16 classification and the
    // 4x4 adaptive probe sample only inside [region_origin,
    // region_origin + region_extent] instead of the full texture. Offsets and
    // extents are in normalized UV space; the renderer derives them from the
    // app-layer modal pixel rect divided by the backdrop texture size. A
    // second dispatch with use_region=1 feeds the modal lane's own state
    // buffer so an open modal judges the desktop directly beneath it while
    // the rest of the UI keeps the global judgment.
    use_region: u32,
    region_origin: vec2<f32>,
    region_extent: vec2<f32>,
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

// --- Strategy 1 (CoverageTuned): constants recalibrated by the
// `darkness_qa` harness sweep (7/12 golden-bucket agreement vs 5/12 for the
// PR #184 literals; see docs/qa/auto-darkness/results.md).
const TUNED_LUMA_LO: f32 = 0.65;
const TUNED_LUMA_HI: f32 = 0.87;
const TUNED_CHROMA_LO: f32 = 0.10;
const TUNED_CHROMA_HI: f32 = 0.26;
const TUNED_COVERAGE_LO: f32 = 0.40;
const TUNED_COVERAGE_HI: f32 = 0.70;

// --- Strategy 2 (ContrastPerceptual): constants.
// Target backdrop-relative luminance the glass tries to hold. Below this
// the backdrop is dark enough that no tint is needed. Issue #186.
const L_TARGET: f32 = 0.20;
// Quantile of the backdrop luminance the darkness answers to: 75 % of
// samples are at or below this luminance.
const QUANTILE: f32 = 0.75;
// 0.75 * 256 samples = 192: cumulative count that defines the quantile bin.
const QUANTILE_TARGET: f32 = QUANTILE * 256.0;
const HISTOGRAM_BIN_COUNT: u32 = 8u;
const HISTOGRAM_BIN_WIDTH: f32 = 1.0 / 8.0;

@group(0) @binding(0) var backdrop_texture: texture_2d<f32>;
@group(0) @binding(1) var backdrop_sampler: sampler;
@group(0) @binding(2) var<uniform> u: AutoDarknessUniforms;
@group(0) @binding(3) var<storage, read_write> state: AutoDarknessState;

var<workgroup> coverage_values: array<f32, 256>;
var<workgroup> probe_values: array<f32, 16>;
var<workgroup> run_full_classification: u32;
// Strategy 2 histogram: 8 atomic bins (zeroed per classification) plus the
// 6-entry reduction tree (4 level-1 partial sums, then 2 level-2 sums).
var<workgroup> histogram_bins: array<atomic<u32>, 8>;
var<workgroup> histogram_partials: array<f32, 6>;

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

fn chroma(rgb: vec3<f32>) -> f32 {
    return max(rgb.r, max(rgb.g, rgb.b)) - min(rgb.r, min(rgb.g, rgb.b));
}

// Strategy 0 baseline; literals intentionally stay inline so the regression
// baseline is byte-for-byte the PR #184 behavior.
fn white_score(rgb: vec3<f32>) -> f32 {
    let bright = smoothstep(0.68, 0.90, luma(rgb));
    let neutral = 1.0 - smoothstep(0.10, 0.26, chroma(rgb));
    return bright * neutral;
}

// Strategy 1: same white-coverage formula, constants hoisted for tuning.
fn tuned_white_score(rgb: vec3<f32>) -> f32 {
    let bright = smoothstep(TUNED_LUMA_LO, TUNED_LUMA_HI, luma(rgb));
    let neutral = 1.0 - smoothstep(TUNED_CHROMA_LO, TUNED_CHROMA_HI, chroma(rgb));
    return bright * neutral;
}

fn srgb_to_linear(c: f32) -> f32 {
    // sRGB transfer function (IEC 61966-2-1). select(f, t, cond) yields t
    // when cond is true, so the low-end branch must be the second argument.
    return select(pow((c + 0.055) / 1.055, 2.4), c / 12.92, c <= 0.04045);
}

// Strategy 2: CIE relative luminance (Rec. 709 primaries).
fn relative_luminance(rgb: vec3<f32>) -> f32 {
    let lin = vec3<f32>(
        srgb_to_linear(rgb.r),
        srgb_to_linear(rgb.g),
        srgb_to_linear(rgb.b),
    );
    return dot(lin, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Statistic per strategy: white-coverage for 0/1, relative luminance for 2.
// Both the 4x4 probe and the full 16x16 classification use this so probe
// wake-ups stay correlated with the classified quantity.
fn strategy_stat(rgb: vec3<f32>) -> f32 {
    if u.strategy == 1u {
        return tuned_white_score(rgb);
    }
    if u.strategy == 2u {
        return relative_luminance(rgb);
    }
    return white_score(rgb);
}

fn interpolated_darkness(now: f32) -> f32 {
    let duration = select(0.30, 0.18, state.target_darkness > state.from_darkness);
    let t = clamp((now - state.transition_started_at) / duration, 0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    return mix(state.from_darkness, state.target_darkness, eased);
}

fn sample_grid(grid_size: u32, x: u32, y: u32) -> vec3<f32> {
    // Full-grid UV in [0, 1] over the bound backdrop texture.
    let uv = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5) / f32(grid_size);
    // Local-region judgment (戦略D): remap the lattice into the modal
    // rectangle. Both the full 16x16 classification and the 4x4 probe use
    // this path, so probe wake-ups stay correlated with the classified
    // region. `region_origin`/`region_extent` are normalized UV coordinates
    // of the modal surface rect (see the uniform struct comment).
    let region_uv = select(
        uv,
        u.region_origin + uv * u.region_extent,
        u.use_region == 1u,
    );
    return textureSampleLevel(backdrop_texture, backdrop_sampler, region_uv, 0.0).rgb;
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
        probe_values[index] = strategy_stat(sample_grid(4u, probe_x, probe_y));
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
    if u.strategy == 2u {
        // Workgroup memory is not zero-initialized, so clear the 8 bins
        // before this classification.
        if index < 8u {
            atomicStore(&histogram_bins[index], 0u);
        }
    } else {
        coverage_values[index] = strategy_stat(rgb);
    }
    workgroupBarrier();

    if u.strategy == 2u {
        let bin = min(
            u32(relative_luminance(rgb) * f32(HISTOGRAM_BIN_COUNT)),
            HISTOGRAM_BIN_COUNT - 1u,
        );
        atomicAdd(&histogram_bins[bin], 1u);
    }
    workgroupBarrier();

    if u.strategy == 2u {
        // Materialize the histogram as f32 for the reduction tree.
        if index < 8u {
            coverage_values[index] = f32(atomicLoad(&histogram_bins[index]));
        }
        workgroupBarrier();

        // Tree reduction: 8 bins -> 4 partial sums (indices 0..4), then 2
        // partial sums stored after them (indices 4..6).
        if index < 4u {
            histogram_partials[index] =
                coverage_values[2u * index] + coverage_values[2u * index + 1u];
        }
        workgroupBarrier();

        if index < 2u {
            histogram_partials[4u + index] =
                histogram_partials[2u * index] + histogram_partials[2u * index + 1u];
        }
        workgroupBarrier();
    } else {
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
    }

    if index == 0u {
        let elapsed = max(u.now - state.last_full_update_at, 0.0);
        let filter_alpha = select(
            1.0 - exp(-elapsed / 0.12),
            1.0,
            state.initialized == 0u,
        );

        var filtered = 0.0;
        var next_target = 0.0;
        if u.strategy == 2u {
            // Quantile search over the reduction tree: skip the darkest
            // 4-bin half when it holds fewer than QUANTILE_TARGET samples,
            // then the darkest 2-bin half, then decide the single bin. The
            // bin center represents the quantile backdrop luminance.
            var quantile_bin = 0u;
            var cumulative = 0.0;
            if cumulative + histogram_partials[4] < QUANTILE_TARGET {
                cumulative += histogram_partials[4];
                quantile_bin = 4u;
            }
            let level1_index = quantile_bin / 2u;
            if cumulative + histogram_partials[level1_index] < QUANTILE_TARGET {
                cumulative += histogram_partials[level1_index];
                quantile_bin += 2u;
            }
            if cumulative + coverage_values[quantile_bin] < QUANTILE_TARGET {
                quantile_bin += 1u;
            }
            let quantile_luma = (f32(quantile_bin) + 0.5) * HISTOGRAM_BIN_WIDTH;

            // EMA-filter the luminance statistic, then invert into the
            // darkness that holds the glass near L_TARGET. The quantile below
            // L_TARGET yields a negative term, clamped to 0 (dark backdrops
            // stay clear); very bright backdrops approach max_darkness.
            filtered = mix(state.filtered_coverage, quantile_luma, filter_alpha);
            next_target = clamp(1.0 - L_TARGET / max(filtered, 1e-4), 0.0, u.max_darkness);
        } else {
            let coverage = coverage_values[0] / 256.0;
            filtered = mix(state.filtered_coverage, coverage, filter_alpha);
            let mapped = select(
                smoothstep(0.55, 0.82, filtered),
                smoothstep(TUNED_COVERAGE_LO, TUNED_COVERAGE_HI, filtered),
                u.strategy == 1u,
            );
            next_target = mapped * clamp(u.max_darkness, 0.0, 1.0);
        }

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
