//! Continuous ScreenCaptureKit backdrop capture for macOS 14 and later.

use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use screencapturekit::cg::CGRect;
use screencapturekit::cm::{CMTime, CVPixelBuffer};
use screencapturekit::prelude::*;
use winit::platform::macos::MonitorHandleExtMacOS;

use crate::app::event::UserEvent;

use super::capture::{
    BackdropCapture, CaptureRegion, CaptureStatus, CpuCaptureFrame, EphemeralGpuCaptureFrame,
    GpuCaptureFrame,
};

const CAPTURE_SCALE_ENV: &str = "LAUNCHPAD_MACOS_CAPTURE_SCALE";
const CAPTURE_QUEUE_DEPTH: u32 = 3;
const DEFAULT_REFRESH_MILLIHERTZ: u32 = 60_000;
const REGION_ALIGNMENT: u32 = 32;
const REGION_HYSTERESIS: u32 = 16;
const BGRA_PIXEL_FORMAT: u32 = u32::from_be_bytes(*b"BGRA");

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

#[derive(Debug, Clone, Copy)]
struct CaptureControl {
    geometry: CaptureGeometry,
    active: bool,
}

struct StreamFrame {
    geometry: CaptureGeometry,
    pixel_buffer: CVPixelBuffer,
}

enum CaptureOutcome {
    Frame(StreamFrame),
    Failed(String),
}

#[derive(Default)]
struct SharedCaptureState {
    configured_geometry: Option<CaptureGeometry>,
    latest: Option<CaptureOutcome>,
}

struct StreamStats {
    last_report_at: Instant,
    frames: u64,
    replaced_frames: u64,
}

impl StreamStats {
    fn new() -> Self {
        Self {
            last_report_at: Instant::now(),
            frames: 0,
            replaced_frames: 0,
        }
    }

    fn record(&mut self, replaced: bool) {
        self.frames += 1;
        self.replaced_frames += u64::from(replaced);

        let elapsed = self.last_report_at.elapsed();
        if elapsed < Duration::from_secs(2) {
            return;
        }
        eprintln!(
            "macOS capture stats: stream_fps={:.1} latest_frame_replacements={} cpu_pixel_copies=0",
            self.frames as f64 / elapsed.as_secs_f64().max(0.001),
            self.replaced_frames,
        );
        self.last_report_at = Instant::now();
        self.frames = 0;
        self.replaced_frames = 0;
    }
}

/// UI-thread endpoint for a persistent ScreenCaptureKit stream. Frame callbacks
/// retain only the newest CVPixelBuffer; Metal imports its IOSurface without a
/// CPU pixel conversion or upload.
pub struct MacOsScreenCapture {
    control_tx: SyncSender<()>,
    control: Arc<Mutex<CaptureControl>>,
    shared: Arc<Mutex<SharedCaptureState>>,
    geometry: CaptureGeometry,
    scale_override: Option<f64>,
    region_initialized: bool,
    active: bool,
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
    let refresh_millihertz = monitor
        .refresh_rate_millihertz()
        .unwrap_or(DEFAULT_REFRESH_MILLIHERTZ)
        .max(1_000);
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
    let control = Arc::new(Mutex::new(CaptureControl {
        geometry,
        active: true,
    }));
    let shared = Arc::new(Mutex::new(SharedCaptureState::default()));
    let (control_tx, control_rx) = mpsc::sync_channel(1);
    let worker_control = Arc::clone(&control);
    let worker_shared = Arc::clone(&shared);
    thread::Builder::new()
        .name("launchpad-macos-capture".to_owned())
        .spawn(move || {
            capture_worker(
                filter,
                geometry,
                refresh_millihertz,
                control_rx,
                worker_control,
                worker_shared,
                event_proxy,
            )
        })
        .map_err(|error| format!("could not start ScreenCaptureKit worker: {error}"))?;

    Ok(Box::new(MacOsScreenCapture {
        control_tx,
        control,
        shared,
        geometry,
        scale_override,
        region_initialized: false,
        active: true,
        fallback_reason: None,
    }))
}

fn capture_worker(
    filter: SCContentFilter,
    initial_geometry: CaptureGeometry,
    refresh_millihertz: u32,
    control_rx: Receiver<()>,
    control: Arc<Mutex<CaptureControl>>,
    shared: Arc<Mutex<SharedCaptureState>>,
    event_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) {
    let mut configuration = SCStreamConfiguration::default();
    configuration.set_pixel_format(PixelFormat::BGRA);
    configuration.set_shows_cursor(false);
    configuration.set_queue_depth(CAPTURE_QUEUE_DEPTH);
    configuration.set_minimum_frame_interval(&CMTime::new(
        1_000,
        refresh_millihertz.min(i32::MAX as u32) as i32,
    ));
    initial_geometry.configure(&mut configuration);

    let callback_stats = Arc::new(Mutex::new(StreamStats::new()));
    let callback_shared = Arc::clone(&shared);
    let callback_proxy = event_proxy.clone();
    let callback_stats_ref = Arc::clone(&callback_stats);
    let mut stream = SCStream::new(&filter, &configuration);
    let handler_id = stream.add_output_handler(
        move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
            if output_type != SCStreamOutputType::Screen
                || sample
                    .get_frame_status()
                    .is_some_and(|status| !status.has_content())
            {
                return;
            }
            let Some(pixel_buffer) = sample.get_image_buffer() else {
                return;
            };
            if pixel_buffer.pixel_format() != BGRA_PIXEL_FORMAT {
                set_failure(
                    &callback_shared,
                    format!(
                        "ScreenCaptureKit returned unsupported pixel format 0x{:08X}",
                        pixel_buffer.pixel_format()
                    ),
                    &callback_proxy,
                );
                return;
            }

            let replaced = {
                let mut state = callback_shared.lock().unwrap();
                let Some(geometry) = state.configured_geometry else {
                    return;
                };
                let (width, height) = geometry.output_size();
                if pixel_buffer.width() != width as usize
                    || pixel_buffer.height() != height as usize
                {
                    return;
                }
                let replaced = matches!(state.latest, Some(CaptureOutcome::Frame(_)));
                state.latest = Some(CaptureOutcome::Frame(StreamFrame {
                    geometry,
                    pixel_buffer,
                }));
                replaced
            };
            callback_stats_ref.lock().unwrap().record(replaced);
            if !replaced {
                let _ = callback_proxy.send_event(UserEvent::BackdropFrameArrived);
            }
        },
        SCStreamOutputType::Screen,
    );
    let Some(handler_id) = handler_id else {
        set_failure(
            &shared,
            "ScreenCaptureKit could not install the screen output handler".to_owned(),
            &event_proxy,
        );
        return;
    };

    shared.lock().unwrap().configured_geometry = Some(initial_geometry);
    if let Err(error) = stream.start_capture() {
        set_failure(
            &shared,
            format!("ScreenCaptureKit stream start failed: {error}"),
            &event_proxy,
        );
        stream.remove_output_handler(handler_id, SCStreamOutputType::Screen);
        return;
    }
    log_geometry(initial_geometry, refresh_millihertz);

    let mut configured_geometry = initial_geometry;
    let mut capturing = true;
    while control_rx.recv().is_ok() {
        let requested = *control.lock().unwrap();
        if requested.geometry != configured_geometry {
            requested.geometry.configure(&mut configuration);
            match stream.update_configuration(&configuration) {
                Ok(()) => {
                    configured_geometry = requested.geometry;
                    let mut state = shared.lock().unwrap();
                    state.configured_geometry = Some(configured_geometry);
                    state.latest = None;
                    drop(state);
                    log_geometry(configured_geometry, refresh_millihertz);
                }
                Err(error) => set_failure(
                    &shared,
                    format!("ScreenCaptureKit configuration update failed: {error}"),
                    &event_proxy,
                ),
            }
        }

        if requested.active != capturing {
            let result = if requested.active {
                stream.start_capture()
            } else {
                stream.stop_capture()
            };
            match result {
                Ok(()) => capturing = requested.active,
                Err(error) => set_failure(
                    &shared,
                    format!("ScreenCaptureKit stream state change failed: {error}"),
                    &event_proxy,
                ),
            }
        }
    }

    if capturing {
        let _ = stream.stop_capture();
    }
    stream.remove_output_handler(handler_id, SCStreamOutputType::Screen);
}

fn set_failure(
    shared: &Arc<Mutex<SharedCaptureState>>,
    reason: String,
    event_proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
) {
    shared.lock().unwrap().latest = Some(CaptureOutcome::Failed(reason));
    let _ = event_proxy.send_event(UserEvent::BackdropFrameArrived);
}

fn log_geometry(geometry: CaptureGeometry, refresh_millihertz: u32) {
    let (output_width, output_height) = geometry.output_size();
    let full_pixels = u64::from(geometry.window_width) * u64::from(geometry.window_height);
    let output_pixels = u64::from(output_width) * u64::from(output_height);
    let reduction = if full_pixels == 0 {
        0.0
    } else {
        100.0 * (1.0 - output_pixels as f64 / full_pixels as f64)
    };
    eprintln!(
        "macOS capture geometry: window={}x{} roi={},{} {}x{} output={}x{} dimension_scale={:.2} target_hz={:.1} pixel_reduction={reduction:.1}%",
        geometry.window_width,
        geometry.window_height,
        geometry.region.x,
        geometry.region.y,
        geometry.region.width,
        geometry.region.height,
        output_width,
        output_height,
        geometry.output_scale,
        refresh_millihertz as f64 / 1_000.0,
    );
}

fn contains(outer: CaptureRegion, inner: CaptureRegion) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.x.saturating_add(outer.width) >= inner.x.saturating_add(inner.width)
        && outer.y.saturating_add(outer.height) >= inner.y.saturating_add(inner.height)
}

fn padded_aligned_region(requested: CaptureRegion, width: u32, height: u32) -> CaptureRegion {
    let requested = requested.clamped_to(width, height);
    let align_down = |value: u32| value / REGION_ALIGNMENT * REGION_ALIGNMENT;
    let align_up = |value: u32, limit: u32| {
        value
            .saturating_add(REGION_ALIGNMENT - 1)
            .saturating_div(REGION_ALIGNMENT)
            .saturating_mul(REGION_ALIGNMENT)
            .min(limit)
    };
    let x = align_down(requested.x.saturating_sub(REGION_HYSTERESIS));
    let y = align_down(requested.y.saturating_sub(REGION_HYSTERESIS));
    let right = align_up(
        requested
            .x
            .saturating_add(requested.width)
            .saturating_add(REGION_HYSTERESIS),
        width.max(1),
    );
    let bottom = align_up(
        requested
            .y
            .saturating_add(requested.height)
            .saturating_add(REGION_HYSTERESIS),
        height.max(1),
    );
    CaptureRegion {
        x,
        y,
        width: right.saturating_sub(x).max(1),
        height: bottom.saturating_sub(y).max(1),
    }
    .clamped_to(width, height)
}

fn stabilize_region(
    current: Option<CaptureRegion>,
    requested: CaptureRegion,
    width: u32,
    height: u32,
) -> CaptureRegion {
    let requested = requested.clamped_to(width, height);
    if let Some(current) = current {
        if contains(current, requested) {
            return current;
        }
    }
    padded_aligned_region(requested, width, height)
}

impl MacOsScreenCapture {
    fn publish_control(&self) {
        *self.control.lock().unwrap() = CaptureControl {
            geometry: self.geometry,
            active: self.active,
        };
        match self.control_tx.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {}
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

    fn set_active(&mut self, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.publish_control();
    }

    fn on_window_moved(&mut self, x: i32, y: i32, scale_factor: f64) {
        self.geometry.window_x = x;
        self.geometry.window_y = y;
        self.geometry.scale_factor = scale_factor;
        self.geometry.output_scale = output_scale(scale_factor, self.scale_override);
        self.publish_control();
    }

    fn set_capture_region(&mut self, region: CaptureRegion) {
        let current = self.region_initialized.then_some(self.geometry.region);
        let next = stabilize_region(
            current,
            region,
            self.geometry.window_width,
            self.geometry.window_height,
        );
        self.region_initialized = true;
        if next != self.geometry.region {
            self.geometry.region = next;
            self.publish_control();
        }
    }

    fn latest_frame_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Option<GpuCaptureFrame> {
        if width == 0 || height == 0 {
            return None;
        }
        if self.geometry.window_width != width || self.geometry.window_height != height {
            self.geometry.window_width = width;
            self.geometry.window_height = height;
            self.geometry.region = self.geometry.region.clamped_to(width, height);
            self.region_initialized = false;
            self.publish_control();
        }

        let outcome = self.shared.lock().unwrap().latest.take();
        match outcome {
            Some(CaptureOutcome::Frame(frame)) if frame.geometry == self.geometry => {
                let (frame_width, frame_height) = frame.geometry.output_size();
                match import_iosurface_texture(device, &frame.pixel_buffer) {
                    Ok(texture) => {
                        self.fallback_reason = None;
                        Some(GpuCaptureFrame::Ephemeral(EphemeralGpuCaptureFrame {
                            texture,
                            region: frame.geometry.region,
                            width: frame_width,
                            height: frame_height,
                            release_after_submit: Box::new(frame.pixel_buffer),
                        }))
                    }
                    Err(reason) => {
                        self.fallback_reason = Some(reason);
                        None
                    }
                }
            }
            Some(CaptureOutcome::Frame(_)) => None,
            Some(CaptureOutcome::Failed(reason)) => {
                self.fallback_reason = Some(reason);
                None
            }
            None => None,
        }
    }

    fn latest_frame_rgba(&mut self, _width: u32, _height: u32) -> Option<CpuCaptureFrame> {
        None
    }
}

fn import_iosurface_texture(
    device: &wgpu::Device,
    pixel_buffer: &CVPixelBuffer,
) -> Result<wgpu::Texture, String> {
    use objc2_io_surface::IOSurfaceRef;
    use objc2_metal::{
        MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureType,
        MTLTextureUsage,
    };
    use wgpu::hal::api::Metal;

    let surface = pixel_buffer
        .get_io_surface()
        .ok_or_else(|| "ScreenCaptureKit frame is not backed by an IOSurface".to_owned())?;
    let width = pixel_buffer.width() as u32;
    let height = pixel_buffer.height() as u32;
    if width == 0 || height == 0 {
        return Err("ScreenCaptureKit returned an empty IOSurface".to_owned());
    }
    let surface_ref = unsafe { &*surface.as_ptr().cast::<IOSurfaceRef>() };
    let hal_device =
        unsafe { device.as_hal::<Metal>() }.ok_or("wgpu device is not using the Metal backend")?;
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::BGRA8Unorm,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setStorageMode(MTLStorageMode::Shared);
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    let raw_texture = hal_device
        .raw_device()
        .newTextureWithDescriptor_iosurface_plane(&descriptor, surface_ref, 0)
        .ok_or_else(|| "Metal could not create a texture from the capture IOSurface".to_owned())?;

    let desc = wgpu::TextureDescriptor {
        label: Some("liquid glass ScreenCaptureKit IOSurface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let hal_texture = unsafe {
        <Metal as wgpu::hal::Api>::Device::texture_from_raw(
            raw_texture,
            desc.format,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        )
    };
    Ok(unsafe { device.create_texture_from_hal::<Metal>(hal_texture, &desc) })
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
            window_width: 800,
            window_height: 600,
            region: CaptureRegion::full(800, 600),
            output_scale: 0.5,
        }
    }

    #[test]
    fn retina_default_halves_each_capture_dimension() {
        assert_eq!(output_scale(2.0, None), 0.5);
        assert_eq!(geometry().output_size(), (400, 300));
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

    #[test]
    fn roi_hysteresis_avoids_reconfiguring_for_small_motion() {
        let first = stabilize_region(
            None,
            CaptureRegion {
                x: 300,
                y: 200,
                width: 200,
                height: 120,
            },
            1_000,
            800,
        );
        let nearby = stabilize_region(
            Some(first),
            CaptureRegion {
                x: 320,
                y: 220,
                width: 200,
                height: 120,
            },
            1_000,
            800,
        );
        assert_eq!(nearby, first);

        let distant = stabilize_region(
            Some(first),
            CaptureRegion {
                x: 700,
                y: 500,
                width: 200,
                height: 120,
            },
            1_000,
            800,
        );
        assert_ne!(distant, first);
        assert!(contains(
            distant,
            CaptureRegion {
                x: 700,
                y: 500,
                width: 200,
                height: 120,
            }
        ));
    }
}
