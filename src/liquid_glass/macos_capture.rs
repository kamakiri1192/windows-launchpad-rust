//! Asynchronous ScreenCaptureKit desktop backdrop for macOS 14 and later.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use screencapturekit::cg::CGRect;
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::SCScreenshotManager;
use winit::platform::macos::MonitorHandleExtMacOS;

use crate::app::event::UserEvent;

use super::capture::{BackdropCapture, CaptureRegion, CaptureStatus, CpuCaptureFrame};

const MAX_CAPTURE_RATE: Duration = Duration::from_millis(33);
const CAPTURE_SCALE_ENV: &str = "LAUNCHPAD_MACOS_CAPTURE_SCALE";

#[derive(Debug, Clone, Copy, PartialEq)]
struct CaptureGeometry {
    monitor_x: i32,
    monitor_y: i32,
    window_x: i32,
    window_y: i32,
    scale_factor: f64,
    window_width: u32,
    window_height: u32,
    region: CaptureRegion,
    output_scale: f64,
}

impl CaptureGeometry {
    fn output_size(self) -> (u32, u32) {
        (
            (f64::from(self.region.width) * self.output_scale)
                .ceil()
                .max(1.0) as u32,
            (f64::from(self.region.height) * self.output_scale)
                .ceil()
                .max(1.0) as u32,
        )
    }

    fn configure(self, configuration: &mut SCStreamConfiguration) {
        let scale = self.scale_factor.max(1.0);
        let source_x =
            f64::from(self.window_x - self.monitor_x) / scale + f64::from(self.region.x) / scale;
        let source_y =
            f64::from(self.window_y - self.monitor_y) / scale + f64::from(self.region.y) / scale;
        let source_width = f64::from(self.region.width) / scale;
        let source_height = f64::from(self.region.height) / scale;
        let (output_width, output_height) = self.output_size();

        configuration.set_width(output_width);
        configuration.set_height(output_height);
        configuration.set_scales_to_fit(true);
        configuration.set_source_rect(CGRect::new(
            source_x.max(0.0),
            source_y.max(0.0),
            source_width,
            source_height,
        ));
    }
}

enum CaptureOutcome {
    Frame {
        geometry: CaptureGeometry,
        frame: CpuCaptureFrame,
    },
    Failed(String),
}

#[derive(Debug)]
struct WorkerStats {
    last_report_at: Instant,
    frames: u32,
    capture_time: Duration,
    conversion_time: Duration,
}

impl WorkerStats {
    fn new() -> Self {
        Self {
            last_report_at: Instant::now(),
            frames: 0,
            capture_time: Duration::ZERO,
            conversion_time: Duration::ZERO,
        }
    }

    fn record(&mut self, capture_time: Duration, conversion_time: Duration) {
        self.frames += 1;
        self.capture_time += capture_time;
        self.conversion_time += conversion_time;

        let elapsed = self.last_report_at.elapsed();
        if elapsed < Duration::from_secs(2) {
            return;
        }

        let seconds = elapsed.as_secs_f32().max(0.001);
        eprintln!(
            "macOS capture stats: capture_fps={:.1} capture_ms={:.2} rgba_copy_ms={:.2}",
            self.frames as f32 / seconds,
            avg_ms(self.capture_time, self.frames),
            avg_ms(self.conversion_time, self.frames),
        );
        *self = Self::new();
    }
}

fn avg_ms(total: Duration, count: u32) -> f32 {
    if count == 0 {
        0.0
    } else {
        total.as_secs_f32() * 1000.0 / count as f32
    }
}

/// UI-thread endpoint for the ScreenCaptureKit worker. A render-frame poll is
/// only a bounded-channel receive/send; the screenshot and ROI RGBA
/// conversion always happen on `launchpad-macos-capture`.
pub struct MacOsScreenCapture {
    request_tx: SyncSender<CaptureGeometry>,
    outcome_rx: Receiver<CaptureOutcome>,
    geometry: CaptureGeometry,
    scale_override: Option<f64>,
    fallback_reason: Option<String>,
}

fn capture_scale_override() -> Option<f64> {
    std::env::var(CAPTURE_SCALE_ENV)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.clamp(0.25, 1.0))
}

fn output_scale(scale_factor: f64, override_scale: Option<f64>) -> f64 {
    override_scale
        .unwrap_or_else(|| 1.0 / scale_factor.max(1.0))
        .clamp(0.25, 1.0)
}

pub fn create_monitor_capture(
    window: &winit::window::Window,
    event_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) -> Result<Box<dyn BackdropCapture>, String> {
    let monitor = window
        .current_monitor()
        .ok_or_else(|| "window has no current monitor".to_owned())?;
    let display_id = monitor.native_id();
    let monitor_position = monitor.position();
    let window_position = window
        .outer_position()
        .map_err(|error| format!("window position unavailable: {error}"))?;

    let content = SCShareableContent::get()
        .map_err(|error| format!("Screen Recording permission/content unavailable: {error}"))?;
    let displays = content.displays();
    let display = displays
        .iter()
        .find(|display| display.display_id() == display_id)
        .or_else(|| displays.first())
        .ok_or_else(|| "ScreenCaptureKit reported no displays".to_owned())?;

    let current_pid = std::process::id() as i32;
    let windows = content.windows();
    let current_windows: Vec<_> = windows
        .iter()
        .filter(|candidate| {
            candidate
                .owning_application()
                .is_some_and(|application| application.process_id() == current_pid)
        })
        .collect();
    let filter = SCContentFilter::builder()
        .display(display)
        .exclude_windows(&current_windows)
        .build();

    let size = window.inner_size();
    let scale_override = capture_scale_override();
    let scale_factor = window.scale_factor();
    let geometry = CaptureGeometry {
        monitor_x: monitor_position.x,
        monitor_y: monitor_position.y,
        window_x: window_position.x,
        window_y: window_position.y,
        scale_factor,
        window_width: size.width.max(1),
        window_height: size.height.max(1),
        region: CaptureRegion::full(size.width, size.height),
        output_scale: output_scale(scale_factor, scale_override),
    };
    let (request_tx, request_rx) = mpsc::sync_channel(1);
    let (outcome_tx, outcome_rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("launchpad-macos-capture".to_owned())
        .spawn(move || capture_worker(filter, request_rx, outcome_tx, event_proxy))
        .map_err(|error| format!("could not start ScreenCaptureKit worker: {error}"))?;

    Ok(Box::new(MacOsScreenCapture {
        request_tx,
        outcome_rx,
        geometry,
        scale_override,
        fallback_reason: None,
    }))
}

fn capture_worker(
    filter: SCContentFilter,
    request_rx: Receiver<CaptureGeometry>,
    outcome_tx: SyncSender<CaptureOutcome>,
    event_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) {
    let mut configuration = SCStreamConfiguration::default();
    configuration.set_pixel_format(PixelFormat::BGRA);
    configuration.set_shows_cursor(false);
    let mut configured_geometry = None;
    let mut previous_capture_started: Option<Instant> = None;
    let mut stats = WorkerStats::new();

    while let Ok(geometry) = request_rx.recv() {
        if let Some(previous) = previous_capture_started {
            let remaining = MAX_CAPTURE_RATE.saturating_sub(previous.elapsed());
            if !remaining.is_zero() {
                thread::sleep(remaining);
            }
        }
        previous_capture_started = Some(Instant::now());

        if configured_geometry != Some(geometry) {
            geometry.configure(&mut configuration);
            configured_geometry = Some(geometry);
            let (output_width, output_height) = geometry.output_size();
            let full_pixels = u64::from(geometry.window_width) * u64::from(geometry.window_height);
            let output_pixels = u64::from(output_width) * u64::from(output_height);
            let reduction = if full_pixels == 0 {
                0.0
            } else {
                100.0 * (1.0 - output_pixels as f64 / full_pixels as f64)
            };
            eprintln!(
                "macOS capture geometry: window={}x{} roi={},{} {}x{} output={}x{} scale={:.2} pixel_reduction={reduction:.1}%",
                geometry.window_width,
                geometry.window_height,
                geometry.region.x,
                geometry.region.y,
                geometry.region.width,
                geometry.region.height,
                output_width,
                output_height,
                geometry.output_scale,
            );
        }

        let capture_started = Instant::now();
        let image = SCScreenshotManager::capture_image(&filter, &configuration);
        let capture_time = capture_started.elapsed();
        let conversion_started = Instant::now();
        let (output_width, output_height) = geometry.output_size();
        let outcome = match image.and_then(|image| image.get_rgba_data()) {
            Ok(pixels) if pixels.len() == output_width as usize * output_height as usize * 4 => {
                CaptureOutcome::Frame {
                    geometry,
                    frame: CpuCaptureFrame {
                        region: geometry.region,
                        width: output_width,
                        height: output_height,
                        pixels,
                    },
                }
            }
            Ok(pixels) => CaptureOutcome::Failed(format!(
                "ScreenCaptureKit returned {} bytes for {}x{}",
                pixels.len(),
                output_width,
                output_height
            )),
            Err(error) => {
                CaptureOutcome::Failed(format!("ScreenCaptureKit capture failed: {error}"))
            }
        };
        let conversion_time = conversion_started.elapsed();
        stats.record(capture_time, conversion_time);

        if outcome_tx.send(outcome).is_err() {
            break;
        }
        if event_proxy
            .send_event(UserEvent::BackdropFrameArrived)
            .is_err()
        {
            break;
        }
    }
}

impl BackdropCapture for MacOsScreenCapture {
    fn status(&self) -> CaptureStatus {
        self.fallback_reason
            .as_ref()
            .map_or(CaptureStatus::Ready, |reason| {
                CaptureStatus::fallback(reason.clone())
            })
    }

    fn on_window_moved(&mut self, x: i32, y: i32, scale_factor: f64) {
        self.geometry.window_x = x;
        self.geometry.window_y = y;
        self.geometry.scale_factor = scale_factor;
        self.geometry.output_scale = output_scale(scale_factor, self.scale_override);
    }

    fn set_capture_region(&mut self, region: CaptureRegion) {
        self.geometry.region =
            region.clamped_to(self.geometry.window_width, self.geometry.window_height);
    }

    fn latest_frame_rgba(&mut self, width: u32, height: u32) -> Option<CpuCaptureFrame> {
        if width == 0 || height == 0 {
            return None;
        }
        self.geometry.window_width = width;
        self.geometry.window_height = height;
        self.geometry.region = self.geometry.region.clamped_to(width, height);

        let outcome = match self.outcome_rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.fallback_reason = Some("ScreenCaptureKit worker stopped".to_owned());
                None
            }
        };

        match self.request_tx.try_send(self.geometry) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                self.fallback_reason = Some("ScreenCaptureKit worker stopped".to_owned());
            }
        }

        match outcome {
            Some(CaptureOutcome::Frame { geometry, frame }) if geometry == self.geometry => {
                self.fallback_reason = None;
                Some(frame)
            }
            Some(CaptureOutcome::Frame { .. }) => None,
            Some(CaptureOutcome::Failed(reason)) => {
                self.fallback_reason = Some(reason);
                None
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> CaptureGeometry {
        CaptureGeometry {
            monitor_x: 0,
            monitor_y: 0,
            window_x: 0,
            window_y: 0,
            scale_factor: 2.0,
            window_width: 4,
            window_height: 4,
            region: CaptureRegion::full(4, 4),
            output_scale: 0.5,
        }
    }

    #[test]
    fn renderer_poll_does_not_wait_for_slow_capture() {
        let (request_tx, request_rx) = mpsc::sync_channel::<CaptureGeometry>(1);
        let (outcome_tx, outcome_rx) = mpsc::sync_channel::<CaptureOutcome>(1);
        let producer = thread::spawn(move || {
            let request = request_rx.recv().expect("capture request");
            thread::sleep(Duration::from_millis(100));
            let (width, height) = request.output_size();
            outcome_tx
                .send(CaptureOutcome::Frame {
                    geometry: request,
                    frame: CpuCaptureFrame {
                        region: request.region,
                        width,
                        height,
                        pixels: vec![0; width as usize * height as usize * 4],
                    },
                })
                .expect("capture result");
        });
        let mut capture = MacOsScreenCapture {
            request_tx,
            outcome_rx,
            geometry: geometry(),
            scale_override: None,
            fallback_reason: None,
        };

        let started = Instant::now();
        assert!(capture.latest_frame_rgba(4, 4).is_none());
        assert!(
            started.elapsed() < Duration::from_millis(40),
            "UI poll inherited worker capture latency"
        );

        producer.join().expect("producer thread");
        let started = Instant::now();
        let frame = capture.latest_frame_rgba(4, 4).unwrap();
        assert_eq!((frame.width, frame.height), (2, 2));
        assert_eq!(frame.pixels.len(), 16);
        assert!(started.elapsed() < Duration::from_millis(40));
    }

    #[test]
    fn retina_capture_uses_one_sample_per_logical_point() {
        assert_eq!(output_scale(2.0, None), 0.5);
        assert_eq!(geometry().output_size(), (2, 2));
        assert_eq!(output_scale(2.0, Some(1.0)), 1.0);
    }

    #[test]
    fn source_rect_offsets_the_roi_inside_the_app_window() {
        let geometry = CaptureGeometry {
            monitor_x: 100,
            monitor_y: 50,
            window_x: 300,
            window_y: 150,
            scale_factor: 2.0,
            window_width: 800,
            window_height: 600,
            region: CaptureRegion {
                x: 40,
                y: 20,
                width: 200,
                height: 100,
            },
            output_scale: 0.5,
        };
        let mut configuration = SCStreamConfiguration::default();
        geometry.configure(&mut configuration);
        let source = configuration.get_source_rect();

        assert_eq!((source.x, source.y), (120.0, 60.0));
        assert_eq!((source.width, source.height), (100.0, 50.0));
        assert_eq!(
            (configuration.get_width(), configuration.get_height()),
            (100, 50)
        );
        assert!(configuration.get_scales_to_fit());
    }
}
