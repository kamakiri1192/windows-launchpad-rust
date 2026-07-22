//! Optional GPU timestamp profiling.
//!
//! The default build compiles the zero-sized no-op implementation below. The
//! real `wgpu-profiler` dependency and timestamp-query device features are only
//! present with `--features gpu-profile`, and runtime collection additionally
//! requires `LAUNCHPAD_GPU_PROFILE=<report.json>`.

#[cfg(feature = "gpu-profile")]
mod enabled {
    use std::collections::{BTreeMap, VecDeque};
    use std::path::{Path, PathBuf};

    use serde_json::json;
    use wgpu_profiler::{GpuProfiler, GpuProfilerQuery, GpuProfilerSettings, GpuTimerQueryResult};

    const PROFILE_ENV: &str = "LAUNCHPAD_GPU_PROFILE";
    const MAX_SAMPLES_PER_SCOPE: usize = 240;

    pub(crate) struct GpuScope(Option<GpuProfilerQuery>);

    #[derive(Default)]
    struct ScopeSamples {
        milliseconds: VecDeque<f64>,
        invalid_samples: u64,
    }

    impl ScopeSamples {
        fn record(&mut self, milliseconds: f64) {
            if milliseconds.is_finite() && milliseconds >= 0.0 {
                self.milliseconds.push_back(milliseconds);
                if self.milliseconds.len() > MAX_SAMPLES_PER_SCOPE {
                    self.milliseconds.pop_front();
                }
            } else {
                self.invalid_samples += 1;
            }
        }
    }

    pub(crate) struct GpuProfilerState {
        profiler: Option<GpuProfiler>,
        report_path: Option<PathBuf>,
        finished_frames: u64,
        samples: BTreeMap<String, ScopeSamples>,
    }

    impl GpuProfilerState {
        pub(crate) fn required_features(adapter_features: wgpu::Features) -> wgpu::Features {
            if profile_path_from_env().is_none() {
                return wgpu::Features::empty();
            }
            adapter_features & GpuProfiler::ALL_WGPU_TIMER_FEATURES
        }

        pub(crate) fn new(device: &wgpu::Device) -> Self {
            let report_path = profile_path_from_env();
            if report_path.is_none() {
                return Self {
                    profiler: None,
                    report_path: None,
                    finished_frames: 0,
                    samples: BTreeMap::new(),
                };
            }
            if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
                eprintln!("gpu profiler disabled: adapter does not support TIMESTAMP_QUERY");
                return Self {
                    profiler: None,
                    report_path,
                    finished_frames: 0,
                    samples: BTreeMap::new(),
                };
            }
            let settings = GpuProfilerSettings {
                enable_timer_queries: true,
                enable_debug_groups: true,
                max_num_pending_frames: 4,
            };
            let profiler = match GpuProfiler::new(device, settings) {
                Ok(profiler) => {
                    eprintln!(
                        "gpu profiler enabled: features={:?} report={}",
                        device.features() & GpuProfiler::ALL_WGPU_TIMER_FEATURES,
                        report_path.as_ref().unwrap().display()
                    );
                    Some(profiler)
                }
                Err(error) => {
                    eprintln!("gpu profiler disabled: {error}");
                    None
                }
            };
            Self {
                profiler,
                report_path,
                finished_frames: 0,
                samples: BTreeMap::new(),
            }
        }

        pub(crate) fn begin(
            &self,
            label: &'static str,
            encoder: &mut wgpu::CommandEncoder,
        ) -> GpuScope {
            GpuScope(
                self.profiler
                    .as_ref()
                    .map(|profiler| profiler.begin_query(label, encoder)),
            )
        }

        pub(crate) fn end(&self, encoder: &mut wgpu::CommandEncoder, scope: GpuScope) {
            if let (Some(profiler), Some(query)) = (self.profiler.as_ref(), scope.0) {
                profiler.end_query(encoder, query);
            }
        }

        pub(crate) fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
            if let Some(profiler) = self.profiler.as_mut() {
                profiler.resolve_queries(encoder);
            }
        }

        pub(crate) fn finish_frame(&mut self, queue: &wgpu::Queue) {
            let Some(profiler) = self.profiler.as_mut() else {
                return;
            };
            if let Err(error) = profiler.end_frame() {
                eprintln!("gpu profiler frame rejected: {error}");
                return;
            }
            let Some(results) = profiler.process_finished_frame(queue.get_timestamp_period())
            else {
                return;
            };
            self.finished_frames += 1;
            collect_samples(&mut self.samples, &results, "");
            if self.finished_frames == 1 || self.finished_frames % 30 == 0 {
                self.write_reports(&results);
            }
        }

        fn write_reports(&self, latest: &[GpuTimerQueryResult]) {
            let Some(path) = self.report_path.as_deref() else {
                return;
            };
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    eprintln!("gpu profiler report directory failed: {error}");
                    return;
                }
            }
            let scopes: serde_json::Map<String, serde_json::Value> = self
                .samples
                .iter()
                .map(|(label, samples)| {
                    let mut values: Vec<_> = samples.milliseconds.iter().copied().collect();
                    values.sort_by(f64::total_cmp);
                    let value = json!({
                        "samples": values.len(),
                        "invalid_samples": samples.invalid_samples,
                        "p50_ms": percentile(&values, 0.50),
                        "p95_ms": percentile(&values, 0.95),
                        "max_ms": values.last().copied().unwrap_or(0.0),
                    });
                    (label.clone(), value)
                })
                .collect();
            let report = json!({
                "schema_version": 2,
                "finished_frames": self.finished_frames,
                "window_samples_per_scope": MAX_SAMPLES_PER_SCOPE,
                "scopes": scopes,
            });
            match serde_json::to_vec_pretty(&report)
                .map_err(|error| error.to_string())
                .and_then(|bytes| std::fs::write(path, bytes).map_err(|error| error.to_string()))
            {
                Ok(()) => eprintln!(
                    "gpu profiler: wrote {} frames to {}",
                    self.finished_frames,
                    path.display()
                ),
                Err(error) => eprintln!("gpu profiler report failed: {error}"),
            }

            let trace_path = trace_path(path);
            if let Err(error) = wgpu_profiler::chrometrace::write_chrometrace(&trace_path, latest) {
                eprintln!("gpu profiler trace failed: {error}");
            }
        }
    }

    fn profile_path_from_env() -> Option<PathBuf> {
        std::env::var_os(PROFILE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    fn trace_path(report_path: &Path) -> PathBuf {
        let stem = report_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("gpu-profile");
        report_path.with_file_name(format!("{stem}.trace.json"))
    }

    fn collect_samples(
        all: &mut BTreeMap<String, ScopeSamples>,
        results: &[GpuTimerQueryResult],
        parent: &str,
    ) {
        for result in results {
            let label = if parent.is_empty() {
                result.label.clone()
            } else {
                format!("{parent}/{}", result.label)
            };
            if let Some(time) = &result.time {
                let scope = all.entry(label.clone()).or_default();
                scope.record((time.end - time.start) * 1000.0);
            }
            collect_samples(all, &result.nested_queries, &label);
        }
    }

    fn percentile(sorted: &[f64], fraction: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
        sorted[index]
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn scope_samples_discard_invalid_gpu_durations() {
            let mut samples = ScopeSamples::default();
            samples.record(0.25);
            samples.record(-1.0);
            samples.record(f64::NAN);

            assert_eq!(
                samples.milliseconds.iter().copied().collect::<Vec<_>>(),
                [0.25]
            );
            assert_eq!(samples.invalid_samples, 2);
        }

        #[test]
        fn scope_samples_keep_a_bounded_recent_window() {
            let mut samples = ScopeSamples::default();
            for index in 0..=MAX_SAMPLES_PER_SCOPE {
                samples.record(index as f64);
            }

            assert_eq!(samples.milliseconds.len(), MAX_SAMPLES_PER_SCOPE);
            assert_eq!(samples.milliseconds.front().copied(), Some(1.0));
        }
    }
}

#[cfg(not(feature = "gpu-profile"))]
mod disabled {
    pub(crate) struct GpuScope;
    pub(crate) struct GpuProfilerState;

    impl GpuProfilerState {
        pub(crate) fn required_features(_adapter_features: wgpu::Features) -> wgpu::Features {
            wgpu::Features::empty()
        }

        pub(crate) fn new(_device: &wgpu::Device) -> Self {
            Self
        }

        pub(crate) fn begin(
            &self,
            _label: &'static str,
            _encoder: &mut wgpu::CommandEncoder,
        ) -> GpuScope {
            GpuScope
        }

        pub(crate) fn end(&self, _encoder: &mut wgpu::CommandEncoder, _scope: GpuScope) {}

        pub(crate) fn resolve(&mut self, _encoder: &mut wgpu::CommandEncoder) {}

        pub(crate) fn finish_frame(&mut self, _queue: &wgpu::Queue) {}
    }
}

#[cfg(not(feature = "gpu-profile"))]
pub(crate) use disabled::*;
#[cfg(feature = "gpu-profile")]
pub(crate) use enabled::*;

#[cfg(test)]
mod tests {
    #[test]
    fn default_build_does_not_request_timestamp_features_without_runtime_opt_in() {
        let requested = super::GpuProfilerState::required_features(wgpu::Features::all());
        if std::env::var_os("LAUNCHPAD_GPU_PROFILE").is_none() {
            assert!(requested.is_empty());
        }
    }
}
