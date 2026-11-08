//! Opt-in wall-clock timings. GPU execution is measured separately by gpu-profile.
//! Batched CSV output keeps per-frame filesystem work out of the measured path.
use std::{io::Write, path::PathBuf, time::Instant};

pub(crate) struct FrameMetrics {
    path: Option<PathBuf>,
    origin: Instant,
    mark: Instant,
    previous_present: Option<Instant>,
    phases: [f64; 8],
    sync_gpu: bool,
    rows: Vec<String>,
    initialized: bool,
    last_flush: Instant,
}

impl FrameMetrics {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self {
            path: std::env::var_os("LAUNCHPAD_FRAME_METRICS").map(PathBuf::from),
            origin: now,
            mark: now,
            previous_present: None,
            phases: [0.0; 8],
            sync_gpu: std::env::var("LAUNCHPAD_PROFILE_SYNC_GPU").as_deref() == Ok("1"),
            rows: Vec::with_capacity(120),
            initialized: false,
            last_flush: now,
        }
    }

    pub(crate) fn begin(&mut self, tick_start: Instant) {
        if self.path.is_some() {
            self.phases = [0.0; 8];
            self.mark = Instant::now();
            self.phases[0] = self.mark.duration_since(tick_start).as_secs_f64() * 1000.0;
        }
    }

    pub(crate) fn mark(&mut self, phase: usize) {
        if self.path.is_some() {
            let now = Instant::now();
            self.phases[phase] += now.duration_since(self.mark).as_secs_f64() * 1000.0;
            self.mark = now;
        }
    }

    pub(crate) fn finish(&mut self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let now = self.mark;
        if let Some(previous) = self.previous_present.replace(now) {
            self.rows.push(format!(
                "{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}\n",
                now.duration_since(self.origin).as_secs_f64(),
                now.duration_since(previous).as_secs_f64() * 1000.0,
                self.phases[0],
                self.phases[1],
                self.phases[2],
                self.phases[3],
                self.phases[4],
                self.phases[5],
                self.phases[6],
                self.phases[7],
            ));
        }
        if self.rows.len() < 120 && now.duration_since(self.last_flush).as_secs_f64() < 2.0 {
            return;
        }
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(self.initialized)
                .truncate(!self.initialized)
                .open(path)?;
            if !self.initialized {
                writeln!(
                    file,
                    "elapsed_s,interval_ms,app_update_ms,acquire_ms,encode_submit_ms,present_ms,gpu_lower_ms,gpu_bar_ms,gpu_upper_ms,gpu_present_ms"
                )?;
            }
            for row in &self.rows {
                file.write_all(row.as_bytes())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("frame metrics: {error}");
        }
        self.initialized = true;
        self.rows.clear();
        self.last_flush = now;
    }

    /// Diagnostic only: destroys CPU/GPU overlap; never use as production FPS.
    /// Includes queueing/driver overhead, not just GPU execution.
    #[inline(never)]
    pub(crate) fn wait_for_gpu(
        &mut self,
        device: &wgpu::Device,
        submission: wgpu::SubmissionIndex,
        phase: usize,
    ) {
        if self.path.is_some() && self.sync_gpu {
            self.mark(2);
            let start = Instant::now();
            if let Err(error) = device.poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            }) {
                eprintln!("frame metrics GPU wait: {error}");
            }
            self.mark = Instant::now();
            self.phases[phase] += self.mark.duration_since(start).as_secs_f64() * 1000.0;
        }
    }
}
