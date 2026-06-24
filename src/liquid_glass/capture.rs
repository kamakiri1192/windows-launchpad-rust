#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStatus {
    Ready,
    Fallback { reason: String },
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
    fn on_window_moved(&mut self) {}
    fn latest_frame_texture(
        &mut self,
        _device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Option<GpuCaptureFrame> {
        None
    }
    fn latest_frame_rgba(&mut self, width: u32, height: u32) -> Option<Vec<u8>>;
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

    fn latest_frame_rgba(&mut self, _width: u32, _height: u32) -> Option<Vec<u8>> {
        None
    }
}
