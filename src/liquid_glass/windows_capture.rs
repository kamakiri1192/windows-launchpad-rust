use super::capture::{BackdropCapture, CaptureStatus, GpuCaptureFrame};
use crate::UserEvent;
use windows::core::Interface;
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[derive(Debug, Clone)]
pub struct WindowCaptureExclusion {
    pub attempted: bool,
    pub success: bool,
    pub message: String,
}

impl WindowCaptureExclusion {
    fn skipped(message: impl Into<String>) -> Self {
        Self {
            attempted: false,
            success: false,
            message: message.into(),
        }
    }

    fn attempted(success: bool, message: impl Into<String>) -> Self {
        Self {
            attempted: true,
            success,
            message: message.into(),
        }
    }
}

pub fn exclude_window_from_capture(window: &winit::window::Window) -> WindowCaptureExclusion {
    let hwnd = match hwnd_from_window(window) {
        Some(hwnd) => hwnd,
        None => return WindowCaptureExclusion::skipped("window handle is not Win32 HWND"),
    };

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR,
    };

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok() {
            return WindowCaptureExclusion::attempted(true, "WDA_EXCLUDEFROMCAPTURE enabled");
        }

        if SetWindowDisplayAffinity(hwnd, WDA_MONITOR).is_ok() {
            return WindowCaptureExclusion::attempted(true, "WDA_MONITOR fallback enabled");
        }
    }

    WindowCaptureExclusion::attempted(false, "SetWindowDisplayAffinity failed")
}

pub fn create_monitor_capture(
    window: &winit::window::Window,
    event_proxy: EventLoopProxy<UserEvent>,
) -> Result<Box<dyn BackdropCapture>, String> {
    WindowsGraphicsCapture::new(window, event_proxy)
        .map(|capture| Box::new(capture) as Box<dyn BackdropCapture>)
}

pub fn enable_system_backdrop_fallback(window: &winit::window::Window) -> Result<(), String> {
    let hwnd = hwnd_from_window(window).ok_or("window handle is not Win32 HWND")?;
    let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void);

    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    };

    let value = DWMSBT_MAINWINDOW.0;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &value as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        )
        .map_err(|e| e.to_string())
    }
}

fn hwnd_from_window(window: &winit::window::Window) -> Option<isize> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn create_capture_session(
    winrt_device: &windows::Graphics::DirectX::Direct3D11::IDirect3DDevice,
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    event_proxy: EventLoopProxy<UserEvent>,
) -> Result<
    (
        windows::Graphics::Capture::GraphicsCaptureItem,
        windows::Graphics::Capture::Direct3D11CaptureFramePool,
        windows::Graphics::Capture::GraphicsCaptureSession,
        i64,
    ),
    String,
> {
    use windows::core::factory;
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

    let item_interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| e.to_string())?;
    let item: GraphicsCaptureItem = unsafe {
        item_interop
            .CreateForMonitor(monitor)
            .map_err(|e| e.to_string())?
    };
    let size = item.Size().map_err(|e| e.to_string())?;
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(|e| e.to_string())?;
    let frame_arrived_token = register_frame_arrived(&frame_pool, event_proxy)?;
    let session = frame_pool
        .CreateCaptureSession(&item)
        .map_err(|e| e.to_string())?;
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);
    session.StartCapture().map_err(|e| e.to_string())?;
    Ok((item, frame_pool, session, frame_arrived_token))
}

fn register_frame_arrived(
    frame_pool: &windows::Graphics::Capture::Direct3D11CaptureFramePool,
    event_proxy: EventLoopProxy<UserEvent>,
) -> Result<i64, String> {
    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::Direct3D11CaptureFramePool;

    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(
        move |_, _| {
            let _ = event_proxy.send_event(UserEvent::BackdropFrameArrived);
            Ok(())
        },
    );
    frame_pool.FrameArrived(&handler).map_err(|e| e.to_string())
}

struct WindowsGraphicsCapture {
    hwnd: windows::Win32::Foundation::HWND,
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    event_proxy: EventLoopProxy<UserEvent>,
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    video_device: windows::Win32::Graphics::Direct3D11::ID3D11VideoDevice,
    video_context: windows::Win32::Graphics::Direct3D11::ID3D11VideoContext,
    _winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice,
    _item: windows::Graphics::Capture::GraphicsCaptureItem,
    frame_pool: windows::Graphics::Capture::Direct3D11CaptureFramePool,
    _session: windows::Graphics::Capture::GraphicsCaptureSession,
    frame_arrived_token: i64,
    staging: Option<StagingTexture>,
    shared: Option<SharedTexture>,
    video_processor: Option<VideoProcessorState>,
    last_frame: Option<Vec<u8>>,
    fallback_reason: Option<String>,
}

struct StagingTexture {
    texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    width: u32,
    height: u32,
}

struct SharedTexture {
    texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    width: u32,
    height: u32,
    imported: bool,
}

struct VideoProcessorState {
    enumerator: windows::Win32::Graphics::Direct3D11::ID3D11VideoProcessorEnumerator,
    processor: windows::Win32::Graphics::Direct3D11::ID3D11VideoProcessor,
    input_width: u32,
    input_height: u32,
    output_width: u32,
    output_height: u32,
}

#[derive(Debug, Clone, Copy)]
struct VideoScaleRequest {
    source_width: u32,
    source_height: u32,
    crop: CropRect,
    output_width: u32,
    output_height: u32,
}

impl WindowsGraphicsCapture {
    fn new(
        window: &winit::window::Window,
        event_proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        use windows::core::Interface;
        use windows::Graphics::Capture::GraphicsCaptureSession;
        use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
        use windows::Win32::Foundation::{HMODULE, HWND};
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            D3D11_SDK_VERSION,
        };
        use windows::Win32::Graphics::Dxgi::IDXGIDevice;
        use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;

        if !GraphicsCaptureSession::IsSupported().map_err(|e| e.to_string())? {
            return Err("Windows.Graphics.Capture is not supported on this OS".to_string());
        }

        let hwnd = hwnd_from_window(window).ok_or("window handle is not Win32 HWND")?;
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_invalid() {
            return Err("MonitorFromWindow failed".to_string());
        }

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| e.to_string())?;
        }

        let device = device.ok_or("D3D11CreateDevice did not return a device")?;
        let context = context.ok_or("D3D11CreateDevice did not return a context")?;
        let video_device = device.cast().map_err(|e| e.to_string())?;
        let video_context = context.cast().map_err(|e| e.to_string())?;
        let dxgi_device: IDXGIDevice = device.cast().map_err(|e| e.to_string())?;
        let inspectable = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device).map_err(|e| e.to_string())?
        };
        let winrt_device: IDirect3DDevice = inspectable.cast().map_err(|e| e.to_string())?;
        let (item, frame_pool, session, frame_arrived_token) =
            create_capture_session(&winrt_device, monitor, event_proxy.clone())?;

        Ok(Self {
            hwnd,
            monitor,
            event_proxy,
            device,
            context,
            video_device,
            video_context,
            _winrt_device: winrt_device,
            _item: item,
            frame_pool,
            _session: session,
            frame_arrived_token,
            staging: None,
            shared: None,
            video_processor: None,
            last_frame: None,
            fallback_reason: None,
        })
    }

    fn recreate_capture_for_monitor(
        &mut self,
        monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    ) -> Result<(), String> {
        let _ = self.frame_pool.RemoveFrameArrived(self.frame_arrived_token);
        let (item, frame_pool, session, frame_arrived_token) =
            create_capture_session(&self._winrt_device, monitor, self.event_proxy.clone())?;
        self.monitor = monitor;
        self._item = item;
        self.frame_pool = frame_pool;
        self._session = session;
        self.frame_arrived_token = frame_arrived_token;
        self.staging = None;
        self.shared = None;
        self.video_processor = None;
        self.last_frame = None;
        self.fallback_reason = None;
        Ok(())
    }

    fn try_latest_frame_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<Option<GpuCaptureFrame>, String> {
        let mut frame = match self.frame_pool.TryGetNextFrame() {
            Ok(frame) => frame,
            Err(_) => return Ok(None),
        };
        while let Ok(newer_frame) = self.frame_pool.TryGetNextFrame() {
            frame = newer_frame;
        }

        let content_size = frame.ContentSize().map_err(|e| e.to_string())?;
        if content_size.Width <= 0 || content_size.Height <= 0 {
            return Ok(None);
        }

        let surface = frame.Surface().map_err(|e| e.to_string())?;
        let access: windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess =
            surface.cast().map_err(|e| e.to_string())?;
        let source: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
            unsafe { access.GetInterface().map_err(|e| e.to_string())? };

        let capture_w = content_size.Width as u32;
        let capture_h = content_size.Height as u32;
        let crop = self.window_crop(capture_w, capture_h)?;
        let width = width.max(1);
        let height = height.max(1);
        self.ensure_shared(width, height)?;

        let shared_texture = self
            .shared
            .as_ref()
            .ok_or("shared texture was not created")?
            .texture
            .clone();
        if crop.width() == width && crop.height() == height {
            copy_crop_to_shared(&self.context, &source, &shared_texture, crop);
        } else {
            self.scale_crop_to_shared(
                &source,
                &shared_texture,
                VideoScaleRequest {
                    source_width: capture_w,
                    source_height: capture_h,
                    crop,
                    output_width: width,
                    output_height: height,
                },
            )?;
        }

        let shared = self
            .shared
            .as_mut()
            .ok_or("shared texture was not created")?;
        if shared.imported {
            return Ok(Some(GpuCaptureFrame::Updated));
        }

        let texture = import_shared_texture(device, &shared.texture, width, height)?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        shared.imported = true;
        Ok(Some(GpuCaptureFrame::New { texture, view }))
    }

    fn ensure_shared(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self
            .shared
            .as_ref()
            .map(|s| s.width == width && s.height == height)
            .unwrap_or(false)
        {
            return Ok(());
        }

        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
            D3D11_CPU_ACCESS_FLAG, D3D11_RESOURCE_MISC_SHARED, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_FLAG(0).0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
        };

        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|e| e.to_string())?;
        }

        self.shared = Some(SharedTexture {
            texture: texture.ok_or("CreateTexture2D returned no shared texture")?,
            width,
            height,
            imported: false,
        });
        Ok(())
    }

    fn ensure_video_processor(
        &mut self,
        input_width: u32,
        input_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Result<(), String> {
        if self
            .video_processor
            .as_ref()
            .map(|vp| {
                vp.input_width == input_width
                    && vp.input_height == input_height
                    && vp.output_width == output_width
                    && vp.output_height == output_height
            })
            .unwrap_or(false)
        {
            return Ok(());
        }

        use windows::Win32::Graphics::Direct3D11::{
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
            D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL;

        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: input_width,
            InputHeight: input_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: output_width,
            OutputHeight: output_height,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };

        let enumerator = unsafe {
            self.video_device
                .CreateVideoProcessorEnumerator(&desc)
                .map_err(|e| format!("CreateVideoProcessorEnumerator failed: {e}"))?
        };
        let processor = unsafe {
            self.video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(|e| format!("CreateVideoProcessor failed: {e}"))?
        };

        self.video_processor = Some(VideoProcessorState {
            enumerator,
            processor,
            input_width,
            input_height,
            output_width,
            output_height,
        });
        Ok(())
    }

    fn scale_crop_to_shared(
        &mut self,
        source: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        dest: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        request: VideoScaleRequest,
    ) -> Result<(), String> {
        self.ensure_video_processor(
            request.source_width,
            request.source_height,
            request.output_width,
            request.output_height,
        )?;
        let processor = self
            .video_processor
            .as_ref()
            .ok_or("video processor was not created")?;
        let result = blit_crop_with_video_processor(
            &self.video_device,
            &self.video_context,
            processor,
            source,
            dest,
            request,
        );
        unsafe {
            self.context.Flush();
        }
        result
    }

    fn try_latest_frame_rgba(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<Vec<u8>>, String> {
        let mut frame = match self.frame_pool.TryGetNextFrame() {
            Ok(frame) => frame,
            Err(_) => return Ok(None),
        };
        while let Ok(newer_frame) = self.frame_pool.TryGetNextFrame() {
            frame = newer_frame;
        }

        let content_size = frame.ContentSize().map_err(|e| e.to_string())?;
        if content_size.Width <= 0 || content_size.Height <= 0 {
            return Ok(None);
        }

        let surface = frame.Surface().map_err(|e| e.to_string())?;
        let access: windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess =
            surface.cast().map_err(|e| e.to_string())?;
        let source: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
            unsafe { access.GetInterface().map_err(|e| e.to_string())? };

        let capture_w = content_size.Width as u32;
        let capture_h = content_size.Height as u32;
        self.ensure_staging(capture_w, capture_h)?;

        let staging = self
            .staging
            .as_ref()
            .ok_or("staging texture was not created")?;
        unsafe {
            self.context.CopyResource(&staging.texture, &source);
        }

        let crop = self.window_crop(capture_w, capture_h)?;
        let rgba = self.read_staging_rgba(staging, width.max(1), height.max(1), crop)?;
        self.last_frame = Some(rgba.clone());
        Ok(Some(rgba))
    }

    fn ensure_staging(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self
            .staging
            .as_ref()
            .map(|s| s.width == width && s.height == height)
            .unwrap_or(false)
        {
            return Ok(());
        }

        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_CPU_ACCESS_READ, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_STAGING,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: D3D11_BIND_FLAG(0).0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|e| e.to_string())?;
        }

        self.staging = Some(StagingTexture {
            texture: texture.ok_or("CreateTexture2D returned no texture")?,
            width,
            height,
        });
        Ok(())
    }

    fn window_crop(&self, capture_w: u32, capture_h: u32) -> Result<CropRect, String> {
        use windows::Win32::Foundation::{POINT, RECT, SIZE};
        use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MONITORINFO};
        use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

        let mut client_rect = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut client_rect).map_err(|e| e.to_string())? };
        let mut top_left = POINT {
            x: client_rect.left,
            y: client_rect.top,
        };
        let mut bottom_right = POINT {
            x: client_rect.right,
            y: client_rect.bottom,
        };
        unsafe {
            if !ClientToScreen(self.hwnd, &mut top_left).as_bool()
                || !ClientToScreen(self.hwnd, &mut bottom_right).as_bool()
            {
                return Err("ClientToScreen failed".to_string());
            }
        }
        let window_rect = RECT {
            left: top_left.x,
            top: top_left.y,
            right: bottom_right.x,
            bottom: bottom_right.y,
        };

        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        let ok = unsafe { GetMonitorInfoW(self.monitor, &mut monitor_info) };
        if !ok.as_bool() {
            return Err("GetMonitorInfoW failed".to_string());
        }

        let monitor_rect = monitor_info.rcMonitor;
        let monitor_size = SIZE {
            cx: (monitor_rect.right - monitor_rect.left).max(1),
            cy: (monitor_rect.bottom - monitor_rect.top).max(1),
        };

        let scale_x = capture_w as f32 / monitor_size.cx as f32;
        let scale_y = capture_h as f32 / monitor_size.cy as f32;
        let left = ((window_rect.left - monitor_rect.left) as f32 * scale_x).floor() as i32;
        let top = ((window_rect.top - monitor_rect.top) as f32 * scale_y).floor() as i32;
        let right = ((window_rect.right - monitor_rect.left) as f32 * scale_x).ceil() as i32;
        let bottom = ((window_rect.bottom - monitor_rect.top) as f32 * scale_y).ceil() as i32;

        Ok(CropRect {
            left: left.clamp(0, capture_w as i32) as u32,
            top: top.clamp(0, capture_h as i32) as u32,
            right: right.clamp(0, capture_w as i32) as u32,
            bottom: bottom.clamp(0, capture_h as i32) as u32,
        })
    }

    fn read_staging_rgba(
        &self,
        staging: &StagingTexture,
        output_w: u32,
        output_h: u32,
        crop: CropRect,
    ) -> Result<Vec<u8>, String> {
        use windows::Win32::Graphics::Direct3D11::{D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ};

        let crop_w = crop.width().max(1);
        let crop_h = crop.height().max(1);
        let resource: windows::Win32::Graphics::Direct3D11::ID3D11Resource =
            staging.texture.cast().map_err(|e| e.to_string())?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();

        unsafe {
            self.context
                .Map(&resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| e.to_string())?;
        }

        let mut out = vec![0u8; (output_w * output_h * 4) as usize];
        let src_base = mapped.pData as *const u8;
        let row_pitch = mapped.RowPitch as usize;

        for y in 0..output_h {
            let src_y = crop.top + ((y as u64 * crop_h as u64) / output_h as u64) as u32;
            let src_y = src_y.min(staging.height - 1);
            for x in 0..output_w {
                let src_x = crop.left + ((x as u64 * crop_w as u64) / output_w as u64) as u32;
                let src_x = src_x.min(staging.width - 1);
                let src_idx = src_y as usize * row_pitch + src_x as usize * 4;
                let dst_idx = ((y * output_w + x) * 4) as usize;
                unsafe {
                    let b = *src_base.add(src_idx);
                    let g = *src_base.add(src_idx + 1);
                    let r = *src_base.add(src_idx + 2);
                    let a = *src_base.add(src_idx + 3);
                    out[dst_idx] = r;
                    out[dst_idx + 1] = g;
                    out[dst_idx + 2] = b;
                    out[dst_idx + 3] = a;
                }
            }
        }

        unsafe {
            self.context.Unmap(&resource, 0);
        }

        Ok(out)
    }
}

impl Drop for WindowsGraphicsCapture {
    fn drop(&mut self) {
        let _ = self.frame_pool.RemoveFrameArrived(self.frame_arrived_token);
    }
}

fn copy_crop_to_shared(
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    source: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    dest: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    crop: CropRect,
) {
    use windows::Win32::Graphics::Direct3D11::{ID3D11Resource, D3D11_BOX};

    let src_resource: ID3D11Resource = match source.cast() {
        Ok(resource) => resource,
        Err(_) => return,
    };
    let dst_resource: ID3D11Resource = match dest.cast() {
        Ok(resource) => resource,
        Err(_) => return,
    };
    let src_box = D3D11_BOX {
        left: crop.left,
        top: crop.top,
        front: 0,
        right: crop.right,
        bottom: crop.bottom,
        back: 1,
    };
    unsafe {
        context.CopySubresourceRegion(&dst_resource, 0, 0, 0, 0, &src_resource, 0, Some(&src_box));
        context.Flush();
    }
}

fn blit_crop_with_video_processor(
    video_device: &windows::Win32::Graphics::Direct3D11::ID3D11VideoDevice,
    video_context: &windows::Win32::Graphics::Direct3D11::ID3D11VideoContext,
    processor: &VideoProcessorState,
    source: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    dest: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    request: VideoScaleRequest,
) -> Result<(), String> {
    use std::mem::ManuallyDrop;
    use std::ptr::null_mut;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Resource, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
        D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
        D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VPIV_DIMENSION_TEXTURE2D,
        D3D11_VPOV_DIMENSION_TEXTURE2D,
    };

    let src_resource: ID3D11Resource = source.cast().map_err(|e| e.to_string())?;
    let dst_resource: ID3D11Resource = dest.cast().map_err(|e| e.to_string())?;

    let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: 0,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPIV {
                MipSlice: 0,
                ArraySlice: 0,
            },
        },
    };
    let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorInputView(
                &src_resource,
                &processor.enumerator,
                &input_desc,
                Some(&mut input_view),
            )
            .map_err(|e| format!("CreateVideoProcessorInputView failed: {e}"))?;
    }

    let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
        },
    };
    let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorOutputView(
                &dst_resource,
                &processor.enumerator,
                &output_desc,
                Some(&mut output_view),
            )
            .map_err(|e| format!("CreateVideoProcessorOutputView failed: {e}"))?;
    }

    let source_rect = RECT {
        left: request.crop.left as i32,
        top: request.crop.top as i32,
        right: request.crop.right as i32,
        bottom: request.crop.bottom as i32,
    };
    let dest_rect = RECT {
        left: 0,
        top: 0,
        right: request.output_width as i32,
        bottom: request.output_height as i32,
    };
    let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
        Enable: true.into(),
        OutputIndex: 0,
        InputFrameOrField: 0,
        PastFrames: 0,
        FutureFrames: 0,
        ppPastSurfaces: null_mut(),
        pInputSurface: ManuallyDrop::new(input_view),
        ppFutureSurfaces: null_mut(),
        ppPastSurfacesRight: null_mut(),
        pInputSurfaceRight: ManuallyDrop::new(None),
        ppFutureSurfacesRight: null_mut(),
    };

    let result = unsafe {
        video_context.VideoProcessorSetStreamFrameFormat(
            &processor.processor,
            0,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        );
        video_context.VideoProcessorSetStreamSourceRect(
            &processor.processor,
            0,
            true,
            Some(&source_rect),
        );
        video_context.VideoProcessorSetStreamDestRect(
            &processor.processor,
            0,
            true,
            Some(&dest_rect),
        );
        video_context.VideoProcessorSetOutputTargetRect(
            &processor.processor,
            true,
            Some(&dest_rect),
        );
        video_context
            .VideoProcessorBlt(
                &processor.processor,
                output_view
                    .as_ref()
                    .ok_or("video processor output view was not created")?,
                0,
                std::slice::from_ref(&stream),
            )
            .map_err(|e| format!("VideoProcessorBlt failed: {e}"))
    };

    unsafe {
        ManuallyDrop::drop(&mut stream.pInputSurface);
        ManuallyDrop::drop(&mut stream.pInputSurfaceRight);
    }
    result
}

fn import_shared_texture(
    device: &wgpu::Device,
    texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<wgpu::Texture, String> {
    use wgpu::hal::api::Dx12;
    use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
    use windows::Win32::Graphics::Dxgi::IDXGIResource;

    let dxgi_resource: IDXGIResource = texture.cast().map_err(|e| e.to_string())?;
    let handle = unsafe { dxgi_resource.GetSharedHandle().map_err(|e| e.to_string())? };
    if handle.is_invalid() {
        return Err("D3D11 shared texture handle is invalid".to_string());
    }

    let hal_device =
        unsafe { device.as_hal::<Dx12>() }.ok_or("wgpu device is not using the DX12 backend")?;
    let mut resource = None::<ID3D12Resource>;
    unsafe {
        hal_device
            .raw_device()
            .OpenSharedHandle(handle, &mut resource)
            .map_err(|e| e.to_string())?;
    }
    let resource = resource.ok_or("OpenSharedHandle returned no D3D12 resource")?;

    let desc = wgpu::TextureDescriptor {
        label: Some("liquid glass shared WGC backdrop"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let hal_texture = unsafe {
        <Dx12 as wgpu::hal::Api>::Device::texture_from_raw(
            resource,
            wgpu::TextureFormat::Bgra8Unorm,
            desc.dimension,
            desc.size,
            desc.mip_level_count,
            desc.sample_count,
        )
    };
    Ok(unsafe { device.create_texture_from_hal::<Dx12>(hal_texture, &desc) })
}

impl BackdropCapture for WindowsGraphicsCapture {
    fn status(&self) -> CaptureStatus {
        if let Some(reason) = self.fallback_reason.as_ref() {
            CaptureStatus::fallback(reason.clone())
        } else {
            CaptureStatus::Ready
        }
    }

    fn on_window_moved(&mut self) {
        use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};

        let monitor = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
        if !monitor.is_invalid() && monitor != self.monitor {
            if let Err(err) = self.recreate_capture_for_monitor(monitor) {
                self.fallback_reason = Some(format!(
                    "window moved to another monitor; capture refresh failed: {err}"
                ));
            }
        }
    }

    fn latest_frame_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Option<GpuCaptureFrame> {
        match self.try_latest_frame_texture(device, width, height) {
            Ok(frame) => {
                self.fallback_reason = None;
                frame
            }
            Err(err) => {
                let reason = format!("GPU capture path failed: {err}");
                if self.fallback_reason.as_deref() != Some(reason.as_str()) {
                    eprintln!("liquid glass capture: GPU texture fallback: {err}");
                }
                self.fallback_reason = Some(reason);
                None
            }
        }
    }

    fn latest_frame_rgba(&mut self, width: u32, height: u32) -> Option<Vec<u8>> {
        match self.try_latest_frame_rgba(width, height) {
            Ok(frame) => {
                self.fallback_reason = None;
                frame
            }
            Err(err) => {
                self.fallback_reason = Some(err);
                self.last_frame.clone()
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CropRect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl CropRect {
    fn width(self) -> u32 {
        self.right.saturating_sub(self.left)
    }

    fn height(self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }
}
