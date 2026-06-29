# Win+Space Global Hotkey Design

This document describes how the resident launcher captures the **Win+Space**
global hotkey and suppresses both the system IME switch and the Start menu,
along with the full record of approaches tried and why each succeeded or
failed. It exists so future maintainers don't repeat the dead ends.

## TL;DR of the working implementation

`src/platform_windows.rs` installs a `WH_KEYBOARD_LL` low-level keyboard hook
on a dedicated thread that owns its own message pump. The hook:

1. Reads the Win-modifier state **live** via `GetAsyncKeyState` (never tracks
   it).
2. Detects fresh Win+Space presses with a **time debounce**
   (`GetTickCount64`, 400 ms) — not a tracked latch.
3. On a genuine Win+Space, swallows the Space keydown, fires
   `UserEvent::Summon`, and **injects a `VK_F20` down/up** so the OS sees an
   intervening keypress between Win-down and Win-up (which stops the shell
   from reading the Win release as a lone tap → no Start menu).
4. **Passes any injected event straight through** (recognized by
   `LLKHF_INJECTED`, our `INJECT_MAGIC` `dwExtraInfo` tag, and an
   `INJECTING` atomic) so our own injected F20 is never re-swallowed.

Win+E / Win+D / lone Win tap all keep working because the hook never touches
the real Win events.

## Requirements

| # | Requirement | How it's met |
|---|-------------|--------------|
| 1 | Win+Space fires a Summon from anywhere | `WH_KEYBOARD_LL` + `EventLoopProxy::send_event(UserEvent::Summon)` |
| 2 | Suppress the system IME switch on the combo | Swallow the Space keydown (`return LRESULT(1)`) so the IME never sees the combo |
| 3 | Don't break other Win+key shortcuts (Win+E etc.) | Never swallow the Win keydown; only Space is swallowed |
| 4 | Don't break the bare Win key (Start menu on a lone tap) | Pass the real Win keydown/keyup through; only inject a harmless dummy key on a consumed combo |
| 5 | Suppress the Start menu that would otherwise pop on the combo | Inject `VK_F20` down/up between the swallowed Space and the Win keyup |
| 6 | No auto-repeat fire (one Summon per press) | Time debounce (400 ms) |
| 7 | No bare-Space false fire | Combo check reads `GetAsyncKeyState(VK_LWIN/RWIN)` live |

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  os-integration thread (owns GetMessageW loop)                │
│                                                               │
│  ┌────────────────────┐    ┌──────────────────────────────┐  │
│  │ WH_KEYBOARD_LL     │    │ tray icon (Shell_NotifyIconW) │  │
│  │ hook_proc          │    │  → popup menu (Show / Quit)   │  │
│  │                    │    └──────────────────────────────┘  │
│  │  on Win+Space:     │                                       │
│  │   SendInput(F20)   │                                       │
│  │   proxy.send_event │──────┐                                │
│  └────────────────────┘      │                                │
└──────────────────────────────┼────────────────────────────────┘
                               │ EventLoopProxy<UserEvent>
                               ▼
┌──────────────────────────────────────────────────────────────┐
│  winit event loop (UI thread)                                 │
│   UserEvent::Summon → summon()  (set_visible + focus_window) │
│   UserEvent::QuitRequested → process::exit(0)                 │
└──────────────────────────────────────────────────────────────┘
```

## Why `WH_KEYBOARD_LL` and not `RegisterHotKey`

`Win+Space` is preempted by the Windows shell for IME switching. A
`RegisterHotKey` call for that combo simply fails (the system reserves it),
and even if it didn't, the IME switch would still fire because
`RegisterHotKey` doesn't suppress the underlying keystroke. A low-level
keyboard hook is the only in-process mechanism that can both detect *and
swallow* the combo before the shell sees it.

## Record of approaches tried

This is the important part — the graveyard of failed attempts, in
chronological order, with the root-cause finding from each. All findings are
backed by the file-backed debug log (`%LOCALAPPDATA%\Launchpad\debug.log`).

### Approach 1 — Track Win down/up, swallow Space only ❌

**Idea:** maintain a `win_down` boolean (set on Win keydown, cleared on Win
keyup); on Space keydown with `win_down`, fire Summon and swallow Space.

**Failed:** a dropped Win keyup (UAC handoff, `LowLevelHooksTimeout`, a
fullscreen app stealing focus, sticky keys, injected events) leaves `win_down`
stuck `true`. After that **every bare Space press fired Summon**, and the
swallowed Space keyup also broke Space in other apps. The debug log proved
the flag was never reset between sessions.

**Lesson:** never track the Win modifier across calls. Read it live with
`GetAsyncKeyState`.

### Approach 2 — Swallow the Win keyup too ❌

**Idea:** once a combo is consumed, also swallow the matching Win keyup so
the OS doesn't see a bare "Win down ... Win up" → no Start menu.

**Failed:** Windows's low-level hook does **not** reliably honor `return 1`
for `WM_KEYUP`/`WM_SYSKEYUP`. The shell's internal Win-tap detector is
independent of the LL-keyup path, so the Start menu still opened on the 2nd+
summon.

**Lesson:** you cannot suppress the Start menu by swallowing the Win keyup.

### Approach 3 — Inject a synthetic Win keyup (`force_win_release`) ❌

**Idea:** on a consumed combo, `SendInput` a Win keyup so the shell believes
Win was released.

**Failed:** timing-dependent; made the Start-menu bug *worse* on the test
machine (the injected keyup and the real one interfered).

**Lesson:** injecting the Win keyup directly is fragile.

### Approach 4 — Swallow the Win **keydown**, re-inject if not Space ❌❌

**Idea:** swallow the Win keydown itself (so the OS can't start a Win-tap
timer). On the next keydown: if Space, it's our combo; if anything else,
re-inject the swallowed Win keydown so Win+E etc. still work.

**Failed catastrophically:** the `SendInput`-injected Win keydown re-entered
our own LL hook and got swallowed again (infinite). The Win key effectively
died: bare Space started firing Summon (a tracked `win_down` stayed true),
Win+E stopped working, and the lone Win tap broke.

This was the worst regression of the whole effort.

**Lesson:** any injected key event will re-enter your own hook. You MUST
recognize your own injections (`LLKHF_INJECTED` / a magic `dwExtraInfo`) and
pass them through.

### Approach 5 — Dummy intervening key (`VK_F24`) ✅ (key choice wrong)

**Idea (the winning strategy):** don't touch the Win key at all. On a
consumed combo, swallow Space and inject a quick `VK_F24` down/up while Win
is still held. The OS now sees "Win down, F24, Win up" — not a lone Win tap —
so the shell doesn't open Start. Win+E etc. stay intact because the real Win
events pass through. Injected events are recognized via `LLKHF_INJECTED` +
`INJECT_MAGIC` + an `INJECTING` atomic and passed through (fixing Approach 4's
re-swallow).

**Key choice was wrong:** `VK_F24` (0x87) triggers the **Windows 11 Snipping
Tool / screenshot** on every summon. Confirmed by the user.

### Approach 6 — Dummy key with `VK_F20` ✅✅ (final)

Same as Approach 5, but the dummy key is `VK_F20` (0x83). Codex consultation
on the safest dummy VK ruled out:

- `VK_F24` — Snipping Tool (this bug)
- `VK_F23` — Copilot key
- `VK_F17` — reserved by the system at shutdown
- `VK_NONCONVERT` / `VK_PROCESSKEY` — IME-related
- `VK_OEM_*` — layout-dependent, some are real Win shortcuts (`Win+.`, `Win+;`)

`VK_F20` is a defined virtual key but is bound to no system shortcut, IME
action, or accessibility feature. **This is the final, working implementation.**

## Two `GetAsyncKeyState` gotchas (both proven by debug logs)

These bit us twice; both are documented in the code comments to prevent a
third time.

1. **Low-order bit ("pressed since last call") is unreliable inside the hook
   callback for the very key being hooked.** It returns 0 even on a genuine
   press. Tried it for fresh-press detection; Summon never fired.
2. **High bit ("currently down") for the key being hooked is also not yet set
   inside its own keydown callback.** Tried `space_held_now()` as an AND
   precondition for firing; every press read stale and Summon never fired.

→ Fresh-press detection uses **only** a `GetTickCount64` time debounce. It
reads no key state, so it can't be poisoned by either gotcha.

## Lifecycle integration (also settled by debug logs)

- **`summon()` focus race:** `SetForegroundWindow` can briefly drop and
  re-acquire focus as the OS shuffles windows during activation; an
  unconditional `Focused(false) → hide()` hid the just-summoned window within
  ~75 ms. Fixed with a `SUMMON_FOCUS_GRACE` (500 ms) window after summon
  during which a `Focused(false)` is ignored.
- **Tray 'Quit' needed two clicks:** `event_loop.exit()` left the
  os-integration thread (and the tray) alive for >1.8 s, so the second click
  landed. Fixed by calling `std::process::exit(0)` on `QuitRequested` for a
  hard, immediate exit (the OS releases the hook + tray icon on process
  teardown).

## Debug logging

Release builds use `windows_subsystem = "windows"` so there's no console and
`eprintln!` goes nowhere. The file-backed logger in `src/debug_logger.rs`
writes timestamped lines to `%LOCALAPPDATA%\Launchpad\debug.log` (truncated
per session), opt-in via the `LAUNCHPAD_DEBUG` environment variable. Every
finding above was established from this log. Run via the `launchpad-debug.bat`
helper, which sets the env var and launches the exe.
