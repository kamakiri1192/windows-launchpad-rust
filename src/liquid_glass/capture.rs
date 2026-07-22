#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStatus {
    Ready,
    Fallback { reason: String },
}

/// Physical-pixel rectangle inside the app window whose desktop backdrop is
/// needed by the Liquid Glass shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRegion {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width: width.max(1),
            height: height.max(1),
        }
    }

    pub fn clamped_to(self, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let x = self.x.min(width - 1);
        let y = self.y.min(height - 1);
        Self {
            x,
            y,
            width: self.width.max(1).min(width - x),
            height: self.height.max(1).min(height - y),
        }
    }

    pub fn pixel_count(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// CPU capture plus the physical window rectangle represented by its pixels.
/// `width`/`height` may be lower than the region extent; the shader maps the
/// texture back over `region`, which lets blurred glass use a cheaper backdrop.
pub struct CpuCaptureFrame {
    pub region: CaptureRegion,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl CpuCaptureFrame {
    pub fn full(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            region: CaptureRegion::full(width, height),
            width: width.max(1),
            height: height.max(1),
            pixels,
        }
    }
}

impl CaptureStatus {
    pub fn fallback(reason: impl Into<String>) -> Self {
        Self::Fallback {
            reason: reason.into(),
        }
    }
}

pub trait BackdropCapture {
    fn status(&self) -> CaptureStatus;
    fn on_window_moved(&mut self, _x: i32, _y: i32, _scale_factor: f64) {}
    fn set_capture_region(&mut self, _region: CaptureRegion) {}
    fn latest_frame_texture(
        &mut self,
        _device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Option<GpuCaptureFrame> {
        None
    }
    fn latest_frame_rgba(&mut self, width: u32, height: u32) -> Option<CpuCaptureFrame>;
}

pub enum GpuCaptureFrame {
    New {
        texture: wgpu::Texture,
        view: wgpu::TextureView,
    },
    Updated,
}

#[derive(Debug)]
pub struct FallbackCapture {
    reason: String,
}

impl FallbackCapture {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl BackdropCapture for FallbackCapture {
    fn status(&self) -> CaptureStatus {
        CaptureStatus::fallback(self.reason.clone())
    }

    fn latest_frame_rgba(&mut self, _width: u32, _height: u32) -> Option<CpuCaptureFrame> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::CaptureRegion;

    #[test]
    fn capture_region_is_kept_inside_the_window() {
        assert_eq!(
            CaptureRegion {
                x: 90,
                y: 70,
                width: 40,
                height: 50,
            }
            .clamped_to(100, 80),
            CaptureRegion {
                x: 90,
                y: 70,
                width: 10,
                height: 10,
            }
        );
    }
}
