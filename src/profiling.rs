//! CPU-side frame profiler for bottleneck identification.
//!
//! Collects per-frame phase timing (via `Instant::now()`) and shape/region
//! counts, then prints aggregated summaries to stderr every `REPORT_INTERVAL`.
//!
//! The profiler is **always compiled in debug builds** (`#[cfg(debug_assertions)]`)
//! and compiles to a zero-sized no-op stub in release builds, avoiding any
//! overhead on the production hot path.

use std::time::Duration;
#[cfg(debug_assertions)]
use std::time::Instant;

/// How often to print a summary line to stderr.
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Phase timings (recorded every frame)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct PhaseStats {
    /// Total accumulated duration since last report.
    total: Duration,
    /// Maximum single-frame duration in this window.
    max: Duration,
    /// Number of frames contributed.
    count: u32,
}

impl PhaseStats {
    fn record(&mut self, dur: Duration) {
        self.total += dur;
        self.max = self.max.max(dur);
        self.count += 1;
    }

    fn avg_ms(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.total.as_secs_f64() * 1000.0 / self.count as f64
    }

    fn max_ms(&self) -> f64 {
        self.max.as_secs_f64() * 1000.0
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Per-frame counts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
struct CountStats {
    total: u64,
    max: u64,
    count: u32,
}

impl CountStats {
    fn record(&mut self, value: u64) {
        self.total += value;
        self.max = self.max.max(value);
        self.count += 1;
    }

    fn avg(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.total as f64 / self.count as f64
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Profiler state
// ---------------------------------------------------------------------------

/// Aggregated profiling data for a frame window.
#[cfg(debug_assertions)]
pub struct FrameProfiler {
    // Timestamps for the *current* frame (set during tick_frame).
    /// Start of `tick_frame` (before scroller tick).
    frame_start: Option<Instant>,
    /// `tick_frame` body (scroller to before render).
    tick_body_start: Option<Instant>,
    /// `render_settings_panel` (build_with_ui + transform).
    settings_build_start: Option<Instant>,

    // Accumulated phase stats.
    tick_body: PhaseStats,
    settings_build: PhaseStats,
    hitmap_clone: PhaseStats,
    tick_scroller: PhaseStats,
    prepare: PhaseStats,
    gpu_render: PhaseStats,

    // Counts.
    overlay_shapes: CountStats,
    modal_shapes: CountStats,
    control_shapes: CountStats,
    base_shapes: CountStats,
    settings_glass_overlay: CountStats,
    settings_glass_modal: CountStats,
    hitmap_regions: CountStats,
    ink_views: CountStats,
    glyph_views: CountStats,
    text_views: CountStats,

    // Report pacing.
    last_report_at: Instant,
    total_frames: u64,
}

#[cfg(debug_assertions)]
impl FrameProfiler {
    pub fn new() -> Self {
        Self {
            frame_start: None,
            tick_body_start: None,
            settings_build_start: None,

            tick_body: PhaseStats::default(),
            settings_build: PhaseStats::default(),
            hitmap_clone: PhaseStats::default(),
            tick_scroller: PhaseStats::default(),
            prepare: PhaseStats::default(),
            gpu_render: PhaseStats::default(),

            overlay_shapes: CountStats::default(),
            modal_shapes: CountStats::default(),
            control_shapes: CountStats::default(),
            base_shapes: CountStats::default(),
            settings_glass_overlay: CountStats::default(),
            settings_glass_modal: CountStats::default(),
            hitmap_regions: CountStats::default(),
            ink_views: CountStats::default(),
            glyph_views: CountStats::default(),
            text_views: CountStats::default(),

            last_report_at: Instant::now(),
            total_frames: 0,
        }
    }

    // ------------------------------------------------------------------
    // Frame lifecycle: called by `tick_frame`
    // ------------------------------------------------------------------

    /// Call at the very top of `tick_frame`.
    pub fn begin_frame(&mut self) {
        self.frame_start = Some(Instant::now());
    }

    /// Call after `ContinuousScroller::tick` and before settings panel render.
    pub fn begin_tick_body(&mut self) {
        self.tick_body_start = Some(Instant::now());
    }

    /// Record scroller tick duration.
    pub fn record_scroller_tick(&mut self, dur: Duration) {
        self.tick_scroller.record(dur);
    }

    /// Call just before `build_with_ui` inside `render_settings_panel`.
    pub fn begin_settings_build(&mut self) {
        self.settings_build_start = Some(Instant::now());
    }

    /// Call after `build_with_ui` + transform + merge completes inside
    /// `render_settings_panel`.
    pub fn end_settings_build(&mut self) {
        if let Some(start) = self.settings_build_start.take() {
            self.settings_build.record(start.elapsed());
        }
    }

    /// Call after `HitMap::clone()`.
    pub fn record_hitmap_clone(&mut self, dur: Duration) {
        self.hitmap_clone.record(dur);
    }

    /// Call after `prepare()`.
    pub fn record_prepare(&mut self, dur: Duration) {
        self.prepare.record(dur);
    }

    /// Call after `render()` (GPU submission; time on CPU side).
    pub fn record_render(&mut self, dur: Duration) {
        self.gpu_render.record(dur);
    }

    /// Call at the end of `tick_frame`.
    pub fn end_frame(&mut self) {
        if let Some(start) = self.tick_body_start.take() {
            self.tick_body.record(start.elapsed());
        }
        self.frame_start = None;
        self.total_frames += 1;
    }

    // ------------------------------------------------------------------
    // Counts: called from render_settings_panel / prepare
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn record_counts(
        &mut self,
        overlay_glass: u64,
        modal_glass: u64,
        control_glass: u64,
        base_glass: u64,
        regions: u64,
        ink: u64,
        glyphs: u64,
        text: u64,
    ) {
        self.overlay_shapes.record(overlay_glass);
        self.modal_shapes.record(modal_glass);
        self.control_shapes.record(control_glass);
        self.base_shapes.record(base_glass);
        self.hitmap_regions.record(regions);
        self.ink_views.record(ink);
        self.glyph_views.record(glyphs);
        self.text_views.record(text);
    }

    // ------------------------------------------------------------------
    // Periodic report
    // ------------------------------------------------------------------

    /// Call every frame. Prints a summary when `REPORT_INTERVAL` has elapsed.
    pub fn maybe_report(&mut self) {
        let elapsed = self.last_report_at.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }

        eprintln!(
            "=== Frame Profiler ({:.1}s, {} frames) ===",
            elapsed.as_secs_f32(),
            self.total_frames,
        );
        eprintln!("  Phase timings (avg / max ms):");
        eprintln!(
            "    scroller_tick    {:7.3} / {:7.3} ms",
            self.tick_scroller.avg_ms(),
            self.tick_scroller.max_ms(),
        );
        eprintln!(
            "    tick_body        {:7.3} / {:7.3} ms",
            self.tick_body.avg_ms(),
            self.tick_body.max_ms(),
        );
        eprintln!(
            "    settings_build   {:7.3} / {:7.3} ms",
            self.settings_build.avg_ms(),
            self.settings_build.max_ms(),
        );
        eprintln!(
            "    hitmap_clone     {:7.3} / {:7.3} ms",
            self.hitmap_clone.avg_ms(),
            self.hitmap_clone.max_ms(),
        );
        eprintln!(
            "    prepare          {:7.3} / {:7.3} ms",
            self.prepare.avg_ms(),
            self.prepare.max_ms(),
        );
        eprintln!(
            "    render (CPU)     {:7.3} / {:7.3} ms",
            self.gpu_render.avg_ms(),
            self.gpu_render.max_ms(),
        );
        eprintln!("  Shape counts (avg / max):");
        eprintln!(
            "    overlay_glass    {:7.1} / {:4}",
            self.overlay_shapes.avg(),
            self.overlay_shapes.max,
        );
        eprintln!(
            "    modal_glass      {:7.1} / {:4}",
            self.modal_shapes.avg(),
            self.modal_shapes.max,
        );
        eprintln!(
            "    control_glass    {:7.1} / {:4}",
            self.control_shapes.avg(),
            self.control_shapes.max,
        );
        eprintln!(
            "    base_glass       {:7.1} / {:4}",
            self.base_shapes.avg(),
            self.base_shapes.max,
        );
        eprintln!("  Model counts (avg / max):");
        eprintln!(
            "    hit_regions      {:7.1} / {:4}",
            self.hitmap_regions.avg(),
            self.hitmap_regions.max,
        );
        eprintln!(
            "    ink_views        {:7.1} / {:4}",
            self.ink_views.avg(),
            self.ink_views.max,
        );
        eprintln!(
            "    glyph_views      {:7.1} / {:4}",
            self.glyph_views.avg(),
            self.glyph_views.max,
        );
        eprintln!(
            "    text_views       {:7.1} / {:4}",
            self.text_views.avg(),
            self.text_views.max,
        );
        eprintln!("  Total frames: {}", self.total_frames,);

        self.reset();
    }

    fn reset(&mut self) {
        self.tick_scroller.reset();
        self.tick_body.reset();
        self.settings_build.reset();
        self.hitmap_clone.reset();
        self.prepare.reset();
        self.gpu_render.reset();

        self.overlay_shapes.reset();
        self.modal_shapes.reset();
        self.control_shapes.reset();
        self.base_shapes.reset();
        self.hitmap_regions.reset();
        self.ink_views.reset();
        self.glyph_views.reset();
        self.text_views.reset();

        self.last_report_at = Instant::now();
    }
}

// ---------------------------------------------------------------------------
// Release-build stub (zero-cost)
// ---------------------------------------------------------------------------

#[cfg(not(debug_assertions))]
pub struct FrameProfiler;

#[cfg(not(debug_assertions))]
impl FrameProfiler {
    pub fn new() -> Self {
        Self
    }
    pub fn begin_frame(&mut self) {}
    pub fn begin_tick_body(&mut self) {}
    pub fn record_scroller_tick(&mut self, _dur: Duration) {}
    pub fn begin_settings_build(&mut self) {}
    pub fn end_settings_build(&mut self) {}
    pub fn record_hitmap_clone(&mut self, _dur: Duration) {}
    pub fn record_prepare(&mut self, _dur: Duration) {}
    pub fn record_render(&mut self, _dur: Duration) {}
    pub fn end_frame(&mut self) {}
    #[allow(clippy::too_many_arguments)]
    pub fn record_counts(
        &mut self,
        _overlay_glass: u64,
        _modal_glass: u64,
        _control_glass: u64,
        _base_glass: u64,
        _regions: u64,
        _ink: u64,
        _glyphs: u64,
        _text: u64,
    ) {
    }
    pub fn maybe_report(&mut self) {}
}
