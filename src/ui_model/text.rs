use crate::ui_model::geometry::{Rect, Size};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::Color;

#[derive(Debug, Clone, PartialEq)]
pub struct TextView {
    pub id: UiId,
    pub text: String,
    pub rect: Rect,
    pub style: TextStyle,
    pub z: i16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub role: TextRole,
    pub size: f32,
    pub color: Color,
    pub weight: TextWeight,
    pub align: TextAlign,
}

impl TextStyle {
    pub const fn new(
        role: TextRole,
        size: f32,
        color: Color,
        weight: TextWeight,
        align: TextAlign,
    ) -> Self {
        Self {
            role,
            size,
            color,
            weight,
            align,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRole {
    AppLabel,
    ControlLabel,
    ControlPlaceholder,
    SettingsTitle,
    SettingsRow,
    FolderTitle,
    FolderItemLabel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextWeight {
    Regular,
    Medium,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextMetrics {
    pub size: Size,
    pub baseline: f32,
}

pub trait TextMeasurer {
    fn measure_line(&mut self, text: &str, style: TextStyle) -> TextMetrics;
}

#[cfg(test)]
mod tests {
    use super::{TextAlign, TextRole, TextStyle, TextWeight};
    use crate::ui_model::render_model::Color;

    #[test]
    fn text_style_carries_semantic_role() {
        let style = TextStyle::new(
            TextRole::AppLabel,
            13.0,
            Color::rgba(1.0, 1.0, 1.0, 1.0),
            TextWeight::Regular,
            TextAlign::Center,
        );

        assert_eq!(style.role, TextRole::AppLabel);
        assert_eq!(style.size, 13.0);
    }
}
