//! Shared application icon asset loading.

/// Canonical launcher icon artwork, used for the window/taskbar icon at
/// runtime. The `.ico` generated from the same source is embedded into the
/// Windows executable by `build.rs`.
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

/// Create the macOS menu-bar glyph at 2× status-bar resolution.
///
/// Menu-bar artwork deliberately does not reuse the colourful application
/// icon: it is a simple 3×3 launchpad grid, with transparent pixels around it.
/// `platform::macos::integration` marks this image as an AppKit template, so
/// macOS tints the opaque glyph appropriately in both light and dark menu
/// bars.
pub fn menu_bar_template_rgba() -> RgbaIcon {
    const SIZE: u32 = 32;
    const FIRST_TILE: f32 = 4.0;
    const TILE_SIZE: f32 = 6.0;
    const STEP: f32 = 9.0;
    const CORNER_RADIUS: f32 = 1.5;
    const SAMPLES_PER_AXIS: u32 = 4;

    let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut covered_samples = 0;
            for sample_y in 0..SAMPLES_PER_AXIS {
                for sample_x in 0..SAMPLES_PER_AXIS {
                    let px = x as f32 + (sample_x as f32 + 0.5) / SAMPLES_PER_AXIS as f32;
                    let py = y as f32 + (sample_y as f32 + 0.5) / SAMPLES_PER_AXIS as f32;
                    let is_covered = (0..3).any(|row| {
                        (0..3).any(|column| {
                            let left = FIRST_TILE + column as f32 * STEP;
                            let top = FIRST_TILE + row as f32 * STEP;
                            point_in_rounded_rect(px, py, left, top, TILE_SIZE, CORNER_RADIUS)
                        })
                    });
                    covered_samples += u32::from(is_covered);
                }
            }

            let alpha = (covered_samples * 255 / (SAMPLES_PER_AXIS * SAMPLES_PER_AXIS)) as u8;
            let index = ((y * SIZE + x) * 4) as usize;
            // Template images are shaped by alpha. Keep the glyph black as a
            // sensible fallback for environments that do not apply tinting.
            rgba[index..index + 4].copy_from_slice(&[0, 0, 0, alpha]);
        }
    }

    RgbaIcon {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}

fn point_in_rounded_rect(x: f32, y: f32, left: f32, top: f32, size: f32, radius: f32) -> bool {
    let center_x = (left + radius).max(x.min(left + size - radius));
    let center_y = (top + radius).max(y.min(top + size - radius));
    let dx = x - center_x;
    let dy = y - center_y;
    dx * dx + dy * dy <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::menu_bar_template_rgba;

    #[test]
    fn menu_bar_icon_is_a_monochrome_transparent_grid() {
        let icon = menu_bar_template_rgba();

        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
        assert_eq!(&icon.rgba[..4], &[0, 0, 0, 0]);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
        assert!(icon
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0));
    }
}
