# Input passthrough implementation

This document records the implementation and repeatable verification for
`INPUT_PASSTHROUGH_REQUIREMENTS.md`. The requirements and technical research
remain the normative behavior and design references.

## Architecture

`src/input_routing.rs` owns the platform-independent state machine:

- one immutable `InputRoutingSnapshot` is published after app state changes;
- the 8 physical-pixel intent threshold resolves left click versus page drag;
- right drag becomes a terminal cancellation;
- wheel forwarding is enabled only while the launcher is visible, idle, and
  the pointer is in `OutsideTransparent`;
- modal, edit, folder, and active gesture states retain launcher ownership.

The native callbacks read a snapshot. They never keep references into the
mutable app, renderer, layout, or window state.

### Windows

The winit message hook intercepts the original `WM_MOUSEWHEEL` and
`WM_POINTERWHEEL`. It walks
downward from the launcher's `GA_ROOT` HWND with `GW_HWNDNEXT`, filters
invisible, disabled, cloaked, transparent, and same-process windows, then
selects one eligible top-level application sink. Resolving from `GA_ROOT` also
covers wheel messages addressed to a focused child/input sink. Hit testing uses
the signed screen point carried by wheel `lParam`, not the separately
DPI-virtualized `MSG.pt`. The unchanged message, `wParam`, and `lParam` are
delivered once. Confirmed clicks continue to use the deepest spatial child.

Chromium and Electron perform their own `WindowFromPoint` check after receiving
`WM_MOUSEWHEEL`; a successful `PostMessageW` can therefore be discarded when
the opaque launcher is still at that point. For the `Chrome_WidgetWin_1`
framework sink, the adapter uses a bounded `SendMessageTimeoutW` dispatch. Only
during that synchronous call, the launcher region has a one-physical-pixel
hit-test hole at the original wheel point; every other pixel remains composed
to avoid visible flashing. Its default region is restored before returning.
This does not hide, deactivate, move, reorder, recreate, or restyle the
launcher. If a future launcher window has a custom region, the compatibility
path returns `Unsupported` instead of overwriting it.

Confirmed clicks resolve and freeze the same target before the launcher hides.
After hide, the adapter verifies that the cursor, root HWND, PID, and deepest
child HWND are unchanged, then uses one private-tagged `SendInput` batch for
the complete button-down/button-up pair. Unlike direct button messages, this
preserves normal Win32 activation, capture, and `WM_CONTEXTMENU` behavior.
Wheel delivery does not call `SetWindowPos`, toggle Z-order, inject a global
mouse stream, or hide/show the launcher.

Before click injection, a harmless targeted `WM_NULL` performs a structured
UIPI preflight because `SendInput` itself does not identify UIPI blocking in
its return value. A target or cursor change cancels delivery instead of
clicking a different window.

Some receivers explicitly activate themselves from their wheel handler. This
is not handled by the rejected broad `forwarding_wheel` / summon-grace
workaround. A wheel queue arms a one-shot correlation before `PostMessageW`
(and removes it again if posting fails), so an immediately activating receiver
cannot race the guard. The correlation contains the launcher foreground state,
exact receiver root HWND, and queue time. A focus-loss auto-hide is suppressed
only when the new foreground window is that exact receiver root within the
bounded interval; the record is consumed on the first focus-loss check.
Unrelated or later focus changes keep normal auto-hide behavior.

### macOS

An AppKit local event monitor observes the original `NSEvent` before winit.
It copies the corresponding `CGEvent`, resolves the window below the launcher
with `windowNumberAtPoint:belowWindowWithWindowNumber:`, maps that window to
its owner PID, writes that exact destination into the two native
window-under-pointer fields, and posts the original event with
`CGEventPostToPid`.
The transparent launcher remains event-opaque (`ignoresMouseEvents = false`);
otherwise AppKit would send the original button event directly to the window
below before the router could distinguish a click from a page drag.

Scroll phase, momentum phase, precise deltas, modifiers, and coordinates remain
part of the copied event. A scroll sequence keeps one target PID. Click
down/up events are retained while the pure router resolves click versus drag;
only a confirmed click is posted. Posted events carry a private source tag and
same-process targets are rejected.

The macOS lifecycle records a requested-focus state for initial creation and
re-summon. Only a `Focused(false)` received before the requested
`Focused(true)` is ignored; after acquisition, unrelated focus loss retains
the normal auto-hide behavior. Native QA snapshots expose the real
`NSWindow::windowNumber`, focused state, and Core Graphics Z-order.

macOS targeted event posting requires Accessibility/Input Monitoring approval.
Permission denial is reported as `DeliveryResult::PermissionDenied`; outside
wheel events are still consumed and never become launcher page input.

## Automated verification

`native_input_probe` creates a separate native GUI process:

- Windows: top-level Win32 window plus nested child;
- macOS: AppKit window plus `NSScrollView`;
- JSONL records include order, button, native deltas, phase/momentum, screen and
  local coordinates, focus/activation, PID, and native window identity.

`input_routing_scenarios` generates physical-equivalent OS input through a path
different from product delivery:

- Windows generator: `SendInput`; product: targeted `PostMessageW` for wheel
  and separately tagged `SendInput` for confirmed clicks;
- macOS generator: HID `CGEventPost`; product: `CGEventPostToPid`.

The product scenarios cover:

1. left click waits for physical up, hides, and delivers one ordered pair;
2. left drag remains visible, moves the page, and delivers no button input;
3. right click waits for physical up, hides, delivers one ordered pair, and
   produces native context-menu dispatch;
4. right drag cancels and delivers nothing;
5. precise vertical wheel remains visible and is delivered once;
6. a test receiver that activates from its wheel handler cannot trigger
   Launchpad auto-hide;
7. hover updates launcher classification and is not delivered;
8. a headful Microsoft Edge page receives a downward wheel and reports an
   actual positive `scrollY`.

The runner also verifies exact wheel sink/root identity, coordinates, modifiers,
launcher PID/window continuity, focus, and Z-order. Windows observes beyond
the bounded receiver-activation interval; macOS compares the real on-screen
window order and native key-window focus before and after wheel delivery. The
macOS probe recognizes the private product tag as an event explicitly posted
to its PID; because AppKit retains the source `NSEvent` window/local metadata,
the single-window probe reports its actual receiver window and converts the
preserved screen point to receiver-local coordinates.
The
one-shot target/time correlation and signed wheel-coordinate extraction are
covered by pure Windows unit tests. The Windows runner temporarily selects
focus-based OS wheel routing and restores the user's previous system setting,
which removes hosted-desktop hover-routing races without sharing the product's
targeted delivery path. The separate Edge compatibility scenario keeps the
machine's real routing setting, opens a guest-mode isolated profile to avoid
first-run/sync overlays, asserts `scrollY > 0`, and verifies the launcher's
window region, style, HWND, PID, visibility, foreground status, and Z-order are
unchanged. Both OS jobs run in
`.github/workflows/input-routing-e2e.yml`.

Local commands:

```text
cargo build --bins
target/debug/input_routing_scenarios
target/debug/input_routing_scenarios --product
target/debug/input_routing_scenarios --browser-compat
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features
```

On Windows, use `.exe` and backslash path separators for the scenario binary.
