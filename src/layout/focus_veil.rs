//! Shared renderer-neutral geometry for the Glass Focus Veil.

use crate::ui_model::geometry::Rect;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{Color, ControlKind, InkView};

/// Cool-neutral tint layered after the scene-space focus blur. Blur carries the
/// visual separation; this restrained wash only lowers residual contrast.
pub const OPACITY: f32 = 0.14;

/// Build the veil that softens the lower scene while a modal is active.
///
/// The rounded geometry deliberately follows the fixed page-frame glass rather
/// than the modal or full window so blur and tint never leak into transparent
/// window regions.
pub fn view(page_frame_rect: Rect, page_frame_radius: f32, progress: f32) -> InkView {
    let progress = progress.clamp(0.0, 1.0);
    let radius = page_frame_radius
        .max(0.0)
        .min(page_frame_rect.width * 0.5)
        .min(page_frame_rect.height * 0.5);

    InkView {
        id: UiId::backdrop("glass-focus-veil"),
        center: page_frame_rect.center(),
        extent: page_frame_rect.height * 0.5,
        opacity: OPACITY * progress,
        scene_blur: progress,
        stroke: page_frame_rect.width * 0.5,
        corner_radius: radius,
        color: Color::rgba(0.12, 0.15, 0.20, 1.0),
        kind: ControlKind::RowBackground,
        z: 90,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::geometry::Point;

    #[test]
    fn veil_tracks_page_frame_and_progress() {
        let veil = view(Rect::new(80.0, 60.0, 1120.0, 680.0), 54.0, 0.5);

        assert_eq!(veil.center, Point::new(640.0, 400.0));
        assert_eq!(veil.stroke, 560.0);
        assert_eq!(veil.extent, 340.0);
        assert_eq!(veil.corner_radius, 54.0);
        assert!((veil.opacity - OPACITY * 0.5).abs() < 0.001);
        assert!((veil.scene_blur - 0.5).abs() < 0.001);
    }
}
