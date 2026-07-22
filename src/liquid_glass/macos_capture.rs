//! ScreenCaptureKit desktop backdrop for macOS 14 and later.

use screencapturekit::cg::CGRect;
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
use winit::platform::macos::MonitorHandleExtMacOS;

use super::capture::{BackdropCapture, CaptureStatus};

/// ScreenCaptureKit filter and geometry for the launcher window's desktop
/// region. Captures are deliberately kept behind the common trait so a denied
/// permission or framework error degrades to the renderer's static fallback.
pub struct MacOsScreenCapture {
    filter: SCContentFilter,
    configuration: SCStreamConfiguration,
    monitor_x: i32,
    monitor_y: i32,
    window_x: i32,
    window_y: i32,
    scale_factor: f64,
    configured_size: (u32, u32),
    fallback_reason: Option<String>,
    last_frame: Option<Vec<u8>>,
}

pub fn create_monitor_capture(
    window: &winit::window::Window,
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
    let filter_builder = SCContentFilter::create().with_display(display);
    let filter = if let Some(application) = current_application {
        filter_builder
            .with_excluding_applications(&[application], &[])
            .build()
    } else {
        filter_builder.with_excluding_windows(&[]).build()
    };

    let size = window.inner_size();
    let mut capture = MacOsScreenCapture {
        filter,
        configuration: SCStreamConfiguration::new()
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(false),
        monitor_x: monitor_position.x,
        monitor_y: monitor_position.y,
        window_x: window_position.x,
        window_y: window_position.y,
        scale_factor: window.scale_factor(),
        configured_size: (0, 0),
        fallback_reason: None,
        last_frame: None,
    };
    capture.configure(size.width.max(1), size.height.max(1));
    Ok(Box::new(capture))
}

impl MacOsScreenCapture {
    fn configure(&mut self, width: u32, height: u32) {
        let scale = self.scale_factor.max(1.0);
        let source_x = f64::from(self.window_x - self.monitor_x) / scale;
        let source_y = f64::from(self.window_y - self.monitor_y) / scale;
        let source_width = f64::from(width) / scale;
        let source_height = f64::from(height) / scale;

        self.configuration
            .set_width(width)
            .set_height(height)
            .set_source_rect(CGRect::new(
                source_x.max(0.0),
                source_y.max(0.0),
                source_width,
                source_height,
            ));
        self.configured_size = (width, height);
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
        self.window_x = x;
        self.window_y = y;
        self.scale_factor = scale_factor;
        self.configured_size = (0, 0);
    }

    fn latest_frame_rgba(&mut self, width: u32, height: u32) -> Option<Vec<u8>> {
        if width == 0 || height == 0 {
            return None;
        }
        if self.configured_size != (width, height) {
            self.configure(width, height);
        }

        let frame = SCScreenshotManager::capture_image(&self.filter, &self.configuration)
            .and_then(|image| image.rgba_data());
        match frame {
            Ok(frame) if frame.len() == width as usize * height as usize * 4 => {
                self.fallback_reason = None;
                self.last_frame = Some(frame.clone());
                Some(frame)
            }
            Ok(frame) => {
                self.fallback_reason = Some(format!(
                    "ScreenCaptureKit returned {} bytes for {width}x{height}",
                    frame.len()
                ));
                self.last_frame.clone()
            }
            Err(error) => {
                self.fallback_reason = Some(format!("ScreenCaptureKit capture failed: {error}"));
                self.last_frame.clone()
            }
        }
    }
}
