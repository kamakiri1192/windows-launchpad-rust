//! Layout container implementations for the immediate-mode `Ui`.
//!
//! Each container saves the current layout context, sets up per-child
//! parameters, runs the body closure, then restores the parent context and
//! advances the parent cursor past the block.

use crate::ui_model::geometry::{Insets, Rect};

use super::context::{LayoutDirection, Ui};
use super::response::Response;

/// Saved layout state that a container restores after its body runs.
struct LayoutSnapshot {
    cursor_x: f32,
    cursor_y: f32,
    direction: LayoutDirection,
    spacing: f32,
    available_width: f32,
    available_height: Option<f32>,
    first_in_container: bool,
}

impl Ui {
    // ------------------------------------------------------------------
    // Public container API
    // ------------------------------------------------------------------

    /// Stack children vertically with the given inter-element `spacing`.
    ///
    /// Returns a `Response` whose `rect` covers all children placed inside
    /// the column.
    pub fn column(&mut self, spacing: f32, body: impl FnOnce(&mut Ui)) -> Response {
        let snap = self.save_layout();
        let start = self.cursor_pos();

        self.direction = LayoutDirection::Vertical;
        self.spacing = spacing;
        self.first_in_container = true;

        body(self);

        let end = self.cursor_pos();
        let block_rect = self.block_rect_vertical(start, end);

        self.restore_layout(snap);
        self.advance_parent_cursor(block_rect);

        self.new_anon_response(block_rect)
    }

    /// Stack children horizontally with the given inter-element `spacing`.
    ///
    /// Returns a `Response` whose `rect` covers all children placed inside
    /// the row.
    pub fn row(&mut self, spacing: f32, body: impl FnOnce(&mut Ui)) -> Response {
        let snap = self.save_layout();
        let start = self.cursor_pos();

        self.direction = LayoutDirection::Horizontal;
        self.spacing = spacing;
        self.first_in_container = true;

        body(self);

        let end = self.cursor_pos();
        let block_rect = self.block_rect_horizontal(start, end);

        self.restore_layout(snap);
        self.advance_parent_cursor(block_rect);

        self.new_anon_response(block_rect)
    }

    /// Advance the layout cursor by `amount` in the current direction.
    ///
    /// Produces no draw data — just moves the insertion point.
    pub fn spacer(&mut self, amount: f32) -> Response {
        self.begin_widget();
        let pos_before = self.cursor_pos();
        match self.direction {
            LayoutDirection::Vertical => {
                self.cursor_y += amount;
                let rect = Rect::new(pos_before.x, pos_before.y, self.available_width, amount);
                self.new_anon_response(rect)
            }
            LayoutDirection::Horizontal => {
                self.cursor_x += amount;
                let rect = Rect::new(pos_before.x, pos_before.y, amount, self.row_height());
                self.new_anon_response(rect)
            }
        }
    }

    /// Run `body` with a clip region pushed onto the clip stack.
    ///
    /// Views pushed by widgets inside `body` automatically inherit this
    /// clip (see `push_ink`, `push_glass`, etc.).
    pub fn with_clip(
        &mut self,
        region: crate::ui_model::geometry::ClipRegion,
        body: impl FnOnce(&mut Ui),
    ) -> Response {
        self.clip_stack.push(region);
        body(self);
        self.clip_stack.pop();
        // with_clip doesn't change cursor layout — return an empty response.
        Response::default()
    }

    /// Add inner padding around `body`: the content area is narrowed by
    /// `insets` and the cursor is offset accordingly.
    pub fn padding(&mut self, insets: Insets, body: impl FnOnce(&mut Ui)) -> Response {
        let snap = self.save_layout();
        let outer_start = self.cursor_pos();

        // Move cursor inward and shrink available area.
        self.cursor_x += insets.left;
        self.cursor_y += insets.top;
        self.available_width -= insets.left + insets.right;
        self.first_in_container = true;
        if let Some(ref mut h) = self.available_height {
            *h -= insets.top + insets.bottom;
        }

        body(self);

        // Cursor is now at the end of inner content; expand back to outer
        // coordinates.
        let inner_end_x = self.cursor_x;
        let inner_end_y = self.cursor_y;
        self.restore_layout(snap);

        // Block rect = outer start .. inner end + insets
        let block_width = (inner_end_x + insets.right) - outer_start.x;
        let block_height = (inner_end_y + insets.bottom) - outer_start.y;
        let block_rect = Rect::new(outer_start.x, outer_start.y, block_width, block_height);

        self.advance_parent_cursor(block_rect);
        self.new_anon_response(block_rect)
    }

    // ------------------------------------------------------------------
    // Internal layout helpers
    // ------------------------------------------------------------------

    fn save_layout(&self) -> LayoutSnapshot {
        LayoutSnapshot {
            cursor_x: self.cursor_x,
            cursor_y: self.cursor_y,
            direction: self.direction,
            spacing: self.spacing,
            available_width: self.available_width,
            available_height: self.available_height,
            first_in_container: self.first_in_container,
        }
    }

    fn restore_layout(&mut self, snap: LayoutSnapshot) {
        self.cursor_x = snap.cursor_x;
        self.cursor_y = snap.cursor_y;
        self.direction = snap.direction;
        self.spacing = snap.spacing;
        self.available_width = snap.available_width;
        self.available_height = snap.available_height;
        self.first_in_container = snap.first_in_container;
    }

    fn cursor_pos(&self) -> crate::ui_model::geometry::Point {
        crate::ui_model::geometry::Point::new(self.cursor_x, self.cursor_y)
    }

    /// Compute the bounding rect for a vertical column block: full available
    /// width, height from `start.y` to `end.y`.
    fn block_rect_vertical(
        &self,
        start: crate::ui_model::geometry::Point,
        end: crate::ui_model::geometry::Point,
    ) -> Rect {
        Rect::new(start.x, start.y, self.available_width, end.y - start.y)
    }

    /// Compute the bounding rect for a horizontal row block: width from
    /// `start.x` to `end.x`, height based on a reasonable default (the row
    /// uses `row_height()` which returns the available content height or a
    /// fallback).
    fn block_rect_horizontal(
        &self,
        start: crate::ui_model::geometry::Point,
        end: crate::ui_model::geometry::Point,
    ) -> Rect {
        let height = self.row_height();
        Rect::new(start.x, start.y, end.x - start.x, height)
    }

    /// Height used for horizontal layout rows.
    fn row_height(&self) -> f32 {
        // Use the smaller of available_height or a sensible default.
        // In Phase 1 we use a fixed row height since widgets don't report
        // their own sizes yet.
        self.available_height.unwrap_or(32.0)
    }

    fn advance_parent_cursor(&mut self, block_rect: Rect) {
        match self.direction {
            LayoutDirection::Vertical => {
                self.cursor_y = block_rect.max_y() + self.spacing;
            }
            LayoutDirection::Horizontal => {
                self.cursor_x = block_rect.max_x() + self.spacing;
            }
        }
    }

    /// Create an anonymous `Response` for a layout container block.
    fn new_anon_response(&mut self, rect: Rect) -> Response {
        let id = self.next_anon_id();
        Response {
            id: id.clone(),
            rect,
            hit_rect: rect,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::context::Ui;
    use crate::ui::theme::Theme;
    use crate::ui_model::geometry::{Insets, Rect};
    use crate::ui_model::ids::UiId;

    fn new_ui() -> Ui {
        Ui::new(Theme::default(), 800.0, 600.0)
    }

    // ------------------------------------------------------------------
    // Cursor progression
    // ------------------------------------------------------------------

    #[test]
    fn spacer_vertical_advances_cursor_y() {
        let mut ui = new_ui();
        let before = ui.cursor_y;
        ui.spacer(42.0);
        assert_eq!(ui.cursor_y, before + 42.0);
    }

    #[test]
    fn spacer_horizontal_advances_cursor_x() {
        let mut ui = new_ui();
        ui.direction = crate::ui::context::LayoutDirection::Horizontal;
        let before = ui.cursor_x;
        ui.spacer(17.0);
        assert_eq!(ui.cursor_x, before + 17.0);
    }

    #[test]
    fn three_spacers_in_column_produce_correct_cursor() {
        let mut ui = new_ui();
        let mut captured_y = 0.0;

        ui.column(8.0, |ui| {
            ui.spacer(10.0);
            ui.spacer(20.0);
            ui.spacer(15.0);
            captured_y = ui.cursor_y;
        });

        // spacing=8 between each: 10 + 8 + 20 + 8 + 15 = 61
        assert_eq!(captured_y, 61.0);
    }

    #[test]
    fn three_spacers_in_row_produce_correct_cursor() {
        let mut ui = new_ui();
        let mut captured_x = 0.0;

        ui.row(4.0, |ui| {
            ui.spacer(5.0);
            ui.spacer(10.0);
            ui.spacer(3.0);
            captured_x = ui.cursor_x;
        });

        // spacing=4 between each: 5 + 4 + 10 + 4 + 3 = 26
        assert_eq!(captured_x, 26.0);
    }

    // ------------------------------------------------------------------
    // Determinism: same inputs → same layout
    // ------------------------------------------------------------------

    #[test]
    fn same_column_layout_produces_same_cursor_position() {
        let run = || {
            let mut ui = new_ui();
            ui.column(5.0, |ui| {
                ui.spacer(10.0);
                ui.spacer(20.0);
            });
            ui.cursor_y
        };
        assert_eq!(run(), run());
    }

    // ------------------------------------------------------------------
    // ID → Rect registration
    // ------------------------------------------------------------------

    #[test]
    fn register_and_retrieve_rect() {
        let mut ui = new_ui();
        let id = UiId::named("hello");
        let rect = Rect::new(1.0, 2.0, 3.0, 4.0);
        ui.register(id.clone(), rect, rect);
        assert_eq!(ui.rect(&id), Some(rect));
    }

    // ------------------------------------------------------------------
    // Padding
    // ------------------------------------------------------------------

    #[test]
    fn padding_narrows_available_width() {
        let mut ui = new_ui();
        let outer_w = ui.available_width;

        ui.padding(Insets::all(10.0), |inner| {
            assert_eq!(inner.available_width, outer_w - 20.0);
            assert_eq!(inner.cursor_x, 10.0);
            assert_eq!(inner.cursor_y, 10.0);
        });
    }

    // ------------------------------------------------------------------
    // Clip stack
    // ------------------------------------------------------------------

    #[test]
    fn with_clip_pushes_and_pops() {
        let mut ui = new_ui();
        assert!(ui.clip_stack.is_empty());

        let clip =
            crate::ui_model::geometry::ClipRegion::new(Rect::new(0.0, 0.0, 100.0, 100.0), 8.0);
        ui.with_clip(clip, |ui| {
            assert_eq!(ui.clip_stack.len(), 1);
            assert_eq!(ui.clip_stack.last(), Some(&clip));
        });
        assert!(ui.clip_stack.is_empty());
    }

    // ------------------------------------------------------------------
    // DPI scale
    // ------------------------------------------------------------------

    #[test]
    fn dpi_scale_factor_stored_in_theme() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let ui = Ui::new(theme, 800.0, 600.0);
        assert_eq!(ui.scale_factor(), 2.0);
    }

    // ------------------------------------------------------------------
    // column / row response rect
    // ------------------------------------------------------------------

    #[test]
    fn column_response_rect_covers_children() {
        let mut ui = new_ui();
        let resp = ui.column(0.0, |ui| {
            ui.spacer(50.0);
            ui.spacer(30.0);
        });
        assert_eq!(resp.rect.y, 0.0);
        assert_eq!(resp.rect.height, 80.0);
        assert_eq!(resp.rect.width, 800.0);
    }

    #[test]
    fn row_response_rect_covers_children() {
        let mut ui = new_ui();
        let resp = ui.row(0.0, |ui| {
            ui.spacer(20.0);
            ui.spacer(30.0);
        });
        assert_eq!(resp.rect.x, 0.0);
        assert_eq!(resp.rect.width, 50.0);
    }
}
