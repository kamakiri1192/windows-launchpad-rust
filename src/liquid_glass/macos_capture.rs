//! Asynchronous ScreenCaptureKit desktop backdrop for macOS 14 and later.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use screencapturekit::cg::CGRect;
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::SCScreenshotManager;
use winit::platform::macos::MonitorHandleExtMacOS;

use crate::app::event::UserEvent;

use super::capture::{BackdropCapture, CaptureStatus};

const MAX_CAPTURE_RATE: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Copy, PartialEq)]
struct CaptureGeometry {
    monitor_x: i32,
    monitor_y: i32,
    window_x: i32,
    window_y: i32,
    scale_factor: f64,
    width: u32,
    height: u32,
}

impl CaptureGeometry {
    fn configure(self, configuration: &mut SCStreamConfiguration) {
        let scale = self.scale_factor.max(1.0);
        let source_x = f64::from(self.window_x - self.monitor_x) / scale;
        let source_y = f64::from(self.window_y - self.monitor_y) / scale;
        let source_width = f64::from(self.width) / scale;
        let source_height = f64::from(self.height) / scale;

        configuration
            .set_width(self.width)
            .set_height(self.height)
            .set_source_rect(CGRect::new(
                source_x.max(0.0),
                source_y.max(0.0),
                source_width,
                source_height,
            ));
    }
}

enum CaptureOutcome {
    Frame {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
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
/// only a bounded-channel receive/send; the screenshot and full-frame RGBA
/// conversion always happen on `launchpad-macos-capture`.
pub struct MacOsScreenCapture {
    request_tx: SyncSender<CaptureGeometry>,
    outcome_rx: Receiver<CaptureOutcome>,
    geometry: CaptureGeometry,
    fallback_reason: Option<String>,
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

    let applications = content.applications();
    let current_application = applications
        .iter()
        .find(|application| application.process_id() == std::process::id() as i32);
    let filter_builder = SCContentFilter::builder().display(display);
    let filter = if let Some(application) = current_application {
        filter_builder
            .exclude_applications(&[application], &[])
            .build()
    } else {
        filter_builder.exclude_windows(&[]).build()
    };

    let size = window.inner_size();
    let geometry = CaptureGeometry {
        monitor_x: monitor_position.x,
        monitor_y: monitor_position.y,
        window_x: window_position.x,
        window_y: window_position.y,
        scale_factor: window.scale_factor(),
        width: size.width.max(1),
        height: size.height.max(1),
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
        fallback_reason: None,
    }))
}

fn capture_worker(
    filter: SCContentFilter,
    request_rx: Receiver<CaptureGeometry>,
    outcome_tx: SyncSender<CaptureOutcome>,
    event_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) {
    let mut configuration = SCStreamConfiguration::new()
        .with_pixel_format(PixelFormat::BGRA)
        .with_shows_cursor(false);
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
        }

        let capture_started = Instant::now();
        let image = SCScreenshotManager::capture_image(&filter, &configuration);
        let capture_time = capture_started.elapsed();
        let conversion_started = Instant::now();
        let outcome = match image.and_then(|image| image.rgba_data()) {
            Ok(pixels)
                if pixels.len() == geometry.width as usize * geometry.height as usize * 4 =>
            {
                CaptureOutcome::Frame {
                    width: geometry.width,
                    height: geometry.height,
                    pixels,
                }
            }
            Ok(pixels) => CaptureOutcome::Failed(format!(
                "ScreenCaptureKit returned {} bytes for {}x{}",
                pixels.len(),
                geometry.width,
                geometry.height
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
    }

    fn latest_frame_rgba(&mut self, width: u32, height: u32) -> Option<Vec<u8>> {
        if width == 0 || height == 0 {
            return None;
        }
        self.geometry.width = width;
        self.geometry.height = height;

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
            Some(CaptureOutcome::Frame {
                width: frame_width,
                height: frame_height,
                pixels,
            }) if (frame_width, frame_height) == (width, height) => {
                self.fallback_reason = None;
                Some(pixels)
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
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn renderer_poll_does_not_wait_for_slow_capture() {
        let (request_tx, request_rx) = mpsc::sync_channel::<CaptureGeometry>(1);
        let (outcome_tx, outcome_rx) = mpsc::sync_channel::<CaptureOutcome>(1);
        let producer = thread::spawn(move || {
            let request = request_rx.recv().expect("capture request");
            thread::sleep(Duration::from_millis(100));
            outcome_tx
                .send(CaptureOutcome::Frame {
                    width: request.width,
                    height: request.height,
                    pixels: vec![0; request.width as usize * request.height as usize * 4],
                })
                .expect("capture result");
        });
        let mut capture = MacOsScreenCapture {
            request_tx,
            outcome_rx,
            geometry: geometry(),
            fallback_reason: None,
        };

        let started = Instant::now();
        assert!(capture.latest_frame_rgba(2, 2).is_none());
        assert!(
            started.elapsed() < Duration::from_millis(40),
            "UI poll inherited worker capture latency"
        );

        producer.join().expect("producer thread");
        let started = Instant::now();
        assert_eq!(capture.latest_frame_rgba(2, 2).unwrap().len(), 16);
        assert!(started.elapsed() < Duration::from_millis(40));
    }
}
