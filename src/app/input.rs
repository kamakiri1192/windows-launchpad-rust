//! Pure input routing: `WindowEvent`/`UserEvent` → routing decision.
//!
//! This module contains only pure functions: given the current shell flags
//! (settings open, editing, control wants keyboard, preedit state) and the raw
//! event data, they return a [`KeyboardRoute`], [`PressRoute`], or
//! [`ReleaseRoute`] that describes *what should happen*. The handler turns
//! those routes into method calls on the update/command/frame layers.
//!
//! These functions re-express the precedence rules the historical
//! `WindowEvent`/`MouseInput` handler arms used, so the rules become
//! deterministic and unit-testable without a window, renderer, or scroller:
//!
//! - Keyboard precedence: settings Esc > edit Esc > search field > launcher
//!   hide > debug keys.
//! - Pointer press precedence: settings overlay > bottom control > edit grid
//!   > normal grid press.
//! - Pointer release precedence: settings overlay > control > edit drop >
//!   pending press (outside passthrough > launch) > scroller release.
//!
//! Behavior preservation: each function is the exact branch order the
//! historical handler used. The handler still owns the side effects; this
//! module only owns the precedence decision.

use winit::keyboard::KeyCode;

use super::event::{KeyboardRoute, PressRoute};
use super::state::SettingsPressTarget;

/// Decide how a pressed key should route.
///
/// Mirrors the historical `WindowEvent::KeyboardInput` match order exactly:
/// 1. settings overlay open + Esc → close settings;
/// 2. editing + Esc → exit edit mode;
/// 3. search field wants keyboard → Esc/Backspace/Left/Right/char handling,
///    then fall through to printable text;
/// 4. Esc with nothing open → hide the launcher;
/// 5. `M` → toggle decorations; `R` (only when the field is closed) → reset
///    icon cache; otherwise → Liquid Glass debug key delegation.
///
/// `text` is the `event.text` payload (printable characters), used only when
/// the search field wants keyboard and the preedit is empty.
pub fn keyboard_route(
    settings_open: bool,
    editing: bool,
    control_wants_keyboard: bool,
    preedit_empty: bool,
    key_code: Option<KeyCode>,
    text: Option<&str>,
) -> KeyboardRoute {
    // 1. Settings overlay takes precedence over everything.
    if settings_open && key_code == Some(KeyCode::Escape) {
        return KeyboardRoute::CloseSettings;
    }

    // 2. Edit mode takes precedence over everything except the search field:
    //    this branch sits before `wants_keyboard` so an open search field still
    //    defers to edit-mode Esc.
    if editing && key_code == Some(KeyCode::Escape) {
        return KeyboardRoute::ExitEditMode;
    }

    // 3. While the search field has focus, the control eats most keys.
    if control_wants_keyboard {
        match key_code {
            Some(KeyCode::Escape) => {
                // If the field was open, Esc clears search and closes it
                // instead of hiding the launcher.
                if control_wants_keyboard {
                    return KeyboardRoute::SearchEscClose;
                }
            }
            Some(KeyCode::Backspace) => {
                if preedit_empty {
                    return KeyboardRoute::SearchBackspace;
                }
                // preedit non-empty: the OS IME owns backspace; just redraw.
                return KeyboardRoute::None;
            }
            Some(KeyCode::ArrowLeft) => {
                return KeyboardRoute::SearchLeft;
            }
            Some(KeyCode::ArrowRight) => {
                return KeyboardRoute::SearchRight;
            }
            _ => {}
        }
        // Otherwise, let printable text through (direct, non-IME chars arrive
        // in event.text). Blocked while preedit is non-empty.
        if preedit_empty {
            if let Some(t) = text {
                if !t.is_empty() {
                    return KeyboardRoute::SearchChar(t.to_string());
                }
            }
        }
        // Search field wants keyboard but the key wasn't handled and there was
        // no printable text: fall through to None (the historical code fell out
        // of the `if wants_keyboard` block without returning).
        return KeyboardRoute::None;
    }

    // 4. Esc with no open field: hide the launcher (stay resident).
    if key_code == Some(KeyCode::Escape) {
        return KeyboardRoute::HideLauncher;
    }

    // 5. Debug keys.
    if key_code == Some(KeyCode::KeyM) {
        return KeyboardRoute::ToggleDecorations;
    }
    // R clears the icon cache, but only when the search field is closed (so
    // typing "r" into search isn't hijacked).
    if key_code == Some(KeyCode::KeyR) && !control_wants_keyboard {
        return KeyboardRoute::ResetIcons;
    }

    // 6. Otherwise, delegate to the Liquid Glass debug key handler.
    match key_code {
        Some(k) => KeyboardRoute::LiquidGlassKey(k),
        None => KeyboardRoute::None,
    }
}

/// Decide how a left-button press should route.
///
/// Mirrors the historical `MouseInput::Pressed` match order exactly:
/// settings overlay (swallow) > bottom control (mark pressed_on_control) >
/// edit mode (hide/drag/exit) > normal grid press (begin pending press).
///
/// `over_control` is whether the press started on the bottom-control capsule
/// (or edit gear), i.e. the intent was not `None`.
pub fn pointer_press_route(
    settings_open: bool,
    settings_press_target: SettingsPressTarget,
    over_control: bool,
    editing: bool,
) -> PressRoute {
    if settings_open {
        return PressRoute::Settings(settings_press_target);
    }
    if over_control {
        return PressRoute::Control;
    }
    if editing {
        return PressRoute::EditGrid;
    }
    PressRoute::GridPress
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::SettingsPressTarget::*;

    fn kb_route(
        settings_open: bool,
        editing: bool,
        wants_kb: bool,
        key: Option<KeyCode>,
    ) -> KeyboardRoute {
        keyboard_route(settings_open, editing, wants_kb, true, key, None)
    }

    // ---- keyboard precedence: settings Esc > edit Esc > search Esc > hide ----

    #[test]
    fn settings_esc_closes_settings_even_while_editing() {
        // Settings open + editing + Esc → close settings (not exit edit).
        let route = kb_route(true, true, false, Some(KeyCode::Escape));
        assert_eq!(route, KeyboardRoute::CloseSettings);
    }

    #[test]
    fn settings_esc_takes_precedence_over_search_field() {
        // Settings open + search field wants keyboard + Esc → close settings.
        let route = kb_route(true, false, true, Some(KeyCode::Escape));
        assert_eq!(route, KeyboardRoute::CloseSettings);
    }

    #[test]
    fn edit_esc_exits_edit_mode() {
        let route = kb_route(false, true, false, Some(KeyCode::Escape));
        assert_eq!(route, KeyboardRoute::ExitEditMode);
    }

    #[test]
    fn edit_esc_takes_precedence_over_search_field() {
        // Editing + search field open + Esc → exit edit (the edit branch sits
        // before wants_keyboard).
        let route = kb_route(false, true, true, Some(KeyCode::Escape));
        assert_eq!(route, KeyboardRoute::ExitEditMode);
    }

    #[test]
    fn search_esc_closes_field_not_launcher() {
        let route = kb_route(false, false, true, Some(KeyCode::Escape));
        assert_eq!(route, KeyboardRoute::SearchEscClose);
    }

    #[test]
    fn esc_with_nothing_open_hides_launcher() {
        let route = kb_route(false, false, false, Some(KeyCode::Escape));
        assert_eq!(route, KeyboardRoute::HideLauncher);
    }

    // ---- search field key handling ----

    #[test]
    fn search_backspace_only_when_preedit_empty() {
        // preedit empty → backspace handled.
        assert_eq!(
            keyboard_route(false, false, true, true, Some(KeyCode::Backspace), None),
            KeyboardRoute::SearchBackspace
        );
        // preedit non-empty → OS IME owns backspace; route None (just redraw).
        assert_eq!(
            keyboard_route(false, false, true, false, Some(KeyCode::Backspace), None),
            KeyboardRoute::None
        );
    }

    #[test]
    fn search_arrows_route_to_search() {
        assert_eq!(
            kb_route(false, false, true, Some(KeyCode::ArrowLeft)),
            KeyboardRoute::SearchLeft
        );
        assert_eq!(
            kb_route(false, false, true, Some(KeyCode::ArrowRight)),
            KeyboardRoute::SearchRight
        );
    }

    #[test]
    fn search_printable_char_routes_to_search_char() {
        let route = keyboard_route(false, false, true, true, Some(KeyCode::KeyA), Some("a"));
        assert_eq!(route, KeyboardRoute::SearchChar("a".to_string()));
    }

    #[test]
    fn search_char_blocked_while_preedit_nonempty() {
        // While the IME owns composition, direct char input is blocked.
        let route = keyboard_route(false, false, true, false, Some(KeyCode::KeyA), Some("a"));
        assert_eq!(route, KeyboardRoute::None);
    }

    // ---- debug keys ----

    #[test]
    fn key_m_toggles_decorations() {
        assert_eq!(
            kb_route(false, false, false, Some(KeyCode::KeyM)),
            KeyboardRoute::ToggleDecorations
        );
    }

    #[test]
    fn key_r_resets_icons_only_when_search_closed() {
        // Search field closed → R resets icons.
        assert_eq!(
            kb_route(false, false, false, Some(KeyCode::KeyR)),
            KeyboardRoute::ResetIcons
        );
        // Search field open → R is a normal char (would route to SearchChar if
        // text present; here no text so it falls through to None within the
        // wants_keyboard branch — R does NOT reset icons).
        let route = kb_route(false, false, true, Some(KeyCode::KeyR));
        assert_ne!(route, KeyboardRoute::ResetIcons);
    }

    #[test]
    fn unknown_key_delegates_to_liquid_glass() {
        assert!(matches!(
            kb_route(false, false, false, Some(KeyCode::F1)),
            KeyboardRoute::LiquidGlassKey(_)
        ));
    }

    // ---- pointer press precedence: settings > control > edit/grid ----

    #[test]
    fn pointer_press_settings_takes_precedence() {
        // Settings open → always settings, even if over control / editing.
        let route = pointer_press_route(true, SettingsPressTarget::Outside, true, true);
        assert!(matches!(route, PressRoute::Settings(_)));
    }

    #[test]
    fn pointer_press_control_before_grid() {
        let route = pointer_press_route(false, Inside, true, false);
        assert_eq!(route, PressRoute::Control);
    }

    #[test]
    fn pointer_press_edit_grid_before_normal_press() {
        let route = pointer_press_route(false, Inside, false, true);
        assert_eq!(route, PressRoute::EditGrid);
    }

    #[test]
    fn pointer_press_normal_grid_when_nothing_else() {
        let route = pointer_press_route(false, Inside, false, false);
        assert_eq!(route, PressRoute::GridPress);
    }
}
