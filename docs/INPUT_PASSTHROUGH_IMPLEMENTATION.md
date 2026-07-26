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

The winit message hook intercepts the original `WM_MOUSEWHEEL`. It walks
downward from the launcher with `GW_HWNDNEXT`, filters invisible, disabled,
cloaked, transparent, and same-process windows, then selects the deepest
eligible child. The unchanged `wParam` and `lParam` are queued once with
`PostMessageW`.

Confirmed clicks resolve and freeze the same target before the launcher hides.
After hide, the adapter verifies that the cursor and frozen target are
unchanged, then uses one private-tagged `SendInput` batch for the complete
button-down/button-up pair. Unlike direct button messages, this preserves
normal Win32 activation, capture, and `WM_CONTEXTMENU` behavior. Wheel delivery
does not call `SetWindowPos`, toggle Z-order, inject a global mouse stream, or
hide/show the launcher.

Before click injection, a harmless targeted `WM_NULL` performs a structured
UIPI preflight because `SendInput` itself does not identify UIPI blocking in
its return value. A target or cursor change cancels delivery instead of
clicking a different window.

Some receivers explicitly activate themselves from their wheel handler. This
is not handled by the rejected broad `forwarding_wheel` / summon-grace
workaround. A successful wheel queue records a one-shot correlation containing
the launcher foreground state, exact receiver root HWND, and queue time. A
focus-loss auto-hide is suppressed only when the new foreground window is that
exact receiver root within the bounded interval; the record is consumed on the
first focus-loss check. Unrelated or later focus changes keep normal auto-hide
behavior.

### macOS

An AppKit local event monitor observes the original `NSEvent` before winit.
It copies the corresponding `CGEvent`, resolves the window below the launcher
with `windowNumberAtPoint:belowWindowWithWindowNumber:`, maps that window to
its owner PID, and posts the original event with `CGEventPostToPid`.

Scroll phase, momentum phase, precise deltas, modifiers, and coordinates remain
part of the copied event. A scroll sequence keeps one target PID. Click
down/up events are retained while the pure router resolves click versus drag;
only a confirmed click is posted. Posted events carry a private source tag and
same-process targets are rejected.

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
7. hover updates launcher classification and is not delivered.

The runner also verifies launcher PID/window continuity and, on Windows, wheel
focus and Z-order stability beyond the bounded receiver-activation interval.
The one-shot target/time correlation is covered by a pure Windows unit test.
Both OS jobs run in
`.github/workflows/input-routing-e2e.yml`.

Local commands:

```text
cargo build --bins
target/debug/input_routing_scenarios
target/debug/input_routing_scenarios --product
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features
```

On Windows, use `.exe` and backslash path separators for the scenario binary.
