//! Shared application icon asset loading.

/// Canonical launcher icon artwork, used for the window/taskbar icon and tray
/// icon at runtime. The `.ico` generated from the same source is embedded into
/// the Windows executable by `build.rs`.
const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon-liquid-glass-neutral.png");

pub struct RgbaIcon {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode the bundled app icon as straight RGBA pixels. When `size` is set,
/// the square source artwork is resized to that exact edge length.
pub fn load_rgba(size: Option<u32>) -> Option<RgbaIcon> {
    let img = image::load_from_memory(APP_ICON_PNG).ok()?.into_rgba8();
    let img = match size {
        Some(size) if img.width() != size || img.height() != size => {
            image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
        }
        _ => img,
    };

    Some(RgbaIcon {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}
