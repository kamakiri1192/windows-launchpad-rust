//! macOS trackpad scroll direction detection.
//!
//! winit 0.30 hands us the raw `scrollingDeltaX/Y` from `NSEvent` without
//! applying the user's "natural scrolling" preference (it does not consult
//! `isDirectionInvertedFromDevice`). homepad inverts the sign via that property,
//! so to match its feel we read the same preference ourselves and invert the
//! horizontal trackpad delta when natural scrolling is on (the macOS default).

use objc2_foundation::{NSString, NSUserDefaults};

/// True when the user has "Natural scrolling" enabled in System Settings
/// (`com.apple.swipescrolldirection` defaults to `true` on modern macOS).
///
/// On non-main threads or if the preference can't be read, this conservatively
/// returns `true` (the macOS default), so a typical install inverts the delta.
pub fn natural_scroll_enabled() -> bool {
    // NSUserDefaults is safe to query from the main thread; the event handler
    // that calls this always runs there.
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str("com.apple.swipescrolldirection");
    // `boolForKey:` returns the registered default (YES) when unset, matching
    // macOS behavior where natural scrolling is on out of the box.
    defaults.boolForKey(&key)
}
