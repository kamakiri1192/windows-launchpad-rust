//! macOS trackpad scroll preference metadata.
//!
//! AppKit has already applied the user's preference to `scrollingDeltaX/Y`,
//! and winit forwards those values. This setting may be recorded for fallback
//! diagnostics, but must never be used to invert an event delta again.

use objc2_foundation::{NSString, NSUserDefaults};

/// True when the user has "Natural scrolling" enabled in System Settings
/// (`com.apple.swipescrolldirection` defaults to `true` on modern macOS).
///
/// This is metadata only; callers must preserve the OS-provided delta sign.
pub fn natural_scroll_enabled() -> bool {
    // NSUserDefaults is safe to query from the main thread; the event handler
    // that calls this always runs there.
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str("com.apple.swipescrolldirection");
    defaults.boolForKey(&key)
}
