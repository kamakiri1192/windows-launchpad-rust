//! Shared application icon asset loading.

/// Canonical launcher icon artwork, used for the window/taskbar icon at
/// runtime. The `.ico` generated from the same source is embedded into the
/// Windows executable by `build.rs`.
const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon-liquid-glass-neutral.png");
const MENU_BAR_ICON_PNG: &[u8] = include_bytes!("../assets/macos/menu-bar-icon-template.png");

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

/// Load the dedicated macOS menu-bar glyph at 2× status-bar resolution.
///
/// Menu-bar artwork deliberately does not reuse the colourful application
/// icon. A rounded outline preserves its glass frame while four solid tiles
/// communicate the launcher function at status-bar size.
/// `platform::macos::integration` marks the image as an AppKit template, so
/// macOS tints it appropriately in both light and dark menu bars.
pub fn load_menu_bar_template_rgba() -> Option<RgbaIcon> {
    let image = image::load_from_memory(MENU_BAR_ICON_PNG)
        .ok()?
        .into_rgba8();
    Some(RgbaIcon {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::load_menu_bar_template_rgba;

    #[test]
    fn menu_bar_icon_is_a_monochrome_transparent_template() {
        let icon = load_menu_bar_template_rgba().expect("menu-bar icon should decode");

        assert_eq!((icon.width, icon.height), (36, 36));
        assert_eq!(icon.rgba.len(), 36 * 36 * 4);
        assert_eq!(&icon.rgba[..4], &[0, 0, 0, 0]);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
        assert!(icon
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0));
    }
}
