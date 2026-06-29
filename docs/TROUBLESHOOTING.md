# Troubleshooting Log — Launcher Lifecycle Feature

A record of the bugs hit while implementing the resident-launcher lifecycle
(Win+Space summon, focus-loss hide, tray icon, stay-resident) and how each
was diagnosed and fixed. Every entry is backed by the file-backed debug log
(`%LOCALAPPDATA%\Launchpad\debug.log`); see [HOTKEY_DESIGN.md](HOTKEY_DESIGN.md)
for the architecture and the full list of hotkey approaches.

## How to gather a debug log

Release builds have no console (`windows_subsystem = "windows"`), so
`eprintln!` is invisible. Use the file logger instead:

1. Build (or download the PR review binary).
2. Set `LAUNCHPAD_DEBUG=1` and launch the exe. The helper
   `launchpad-debug.bat` does both.
3. Reproduce the bug.
4. Read `%LOCALAPPDATA%\Launchpad\debug.log` (or `launchpad-debug.log` next to
   the exe if `LOCALAPPDATA` is unavailable). The log is truncated on each
   launch, so it contains only the latest session.

The log records every hook decision, every `summon`/`hide`, every
`Focused(true/false)`, and every tray-menu selection with timestamps. That
ordering is what unmasked almost every bug below.

---

## Bug 1 — Bare Space fires Summon

**Symptom:** pressing Space alone (no Win) sometimes summoned the launcher.

**Root cause:** the hook tracked the Win modifier with a `win_down` boolean.
A dropped Win keyup (UAC handoff, `LowLevelHooksTimeout`, fullscreen focus
steal, sticky keys, injected events) left it stuck `true`, after which every
bare Space matched the "Win held" check.

**Fix:** never track the Win modifier. Read it live with
`GetAsyncKeyState(VK_LWIN/VK_RWIN)` on each Space keydown.

## Bug 2 — Start menu opens on the 2nd Win+Space

**Symptom:** first Win+Space worked; after Esc and a second Win+Space, the
Start menu opened instead.

**Root cause (debug log):** the 2nd Win keydown arrived with `win_down=true,
consumed=true, latched=true` — i.e. state leaked from the *1st* session
because the 1st Win keyup was never delivered to the hook. The 2nd Win+Space
was then treated as an auto-repeat; the real Win keyup passed through and the
shell read it as a lone Win tap → Start menu.

**Fix (after several wrong turns — see HOTKEY_DESIGN.md):** the dummy-key
injection approach. Don't swallow the Win keyup; instead inject a harmless
`VK_F20` down/up between the swallowed Space and the Win keyup so the OS
never sees a lone Win tap.

## Bug 3 — Tray 'Quit' needs two clicks

**Symptom:** right-click → 終了 didn't quit on the first click; the tray was
still clickable ~1.8 s later.

**Root cause (debug log):** `event_loop.exit()` was called, but the
os-integration thread (which owns the tray + hook) kept the process alive
past the exit signal, so the second click landed.

**Fix:** on `UserEvent::QuitRequested`, call `std::process::exit(0)` for a
hard immediate exit. The OS releases the LL hook and removes the tray icon on
process teardown, so no manual `Drop` cleanup is needed.

> Note: the classic tray-popup "first click only dismisses" workaround
> (`SetForegroundWindow` + a posted `WM_NULL` before `TrackPopupMenu`) is
> also relevant if the menu itself ever swallows the first click, but the
> debug log showed this bug was the exit-thread-still-alive one, not the
> popup one.

## Bug 4 — Summoned window vanishes within ~75 ms

**Symptom:** Win+Space reliably summoned, but from the 2nd/3rd+ press the
window disappeared almost instantly.

**Root cause (debug log):**
```
23:48:20.249 Focused(true)      <- summon succeeded
23:48:20.324 Focused(false)     <- 75 ms later, focus dropped
23:48:20.324 hide               <- auto-hide fired
```
`SetForegroundWindow` can briefly lose and re-acquire focus as the OS
shuffles windows during activation. Our unconditional `Focused(false) → hide()`
turned that transient into a self-inflicted dismiss.

**Fix:** record `last_summon` at summon time and ignore any `Focused(false)`
that arrives within `SUMMON_FOCUS_GRACE` (500 ms) of it. Genuine focus loss
(Alt-Tab, clicking another window) happens well after that window and still
hides as expected.

## Bug 5 — Win+Space stops summoning entirely (debounce build)

**Symptom:** after switching to the time-debounce fresh-press detection,
*no* Win+Space press ever fired Summon (debug log: `fresh=false` on every
press).

**Root cause:** I used `GetAsyncKeyState(VK_SPACE)`'s **low-order bit**
("pressed since last call") to detect a fresh press. Per MSDN that bit is
only reliable when read from a *different* thread; reading it inside the hook
callback for the very key being hooked always returns 0.

**Fix:** drop the low-bit check; rely on the time debounce alone for
fresh-press detection. (The high-bit `space_held_now()` check added later was
the same bug in disguise — see Bug 7.)

## Bug 6 — Catastrophic Win-key breakage (swallow-Win-keydown build)

**Symptom:** bare Space fired Summon, Win+E stopped working, lone Win tap
stopped opening Start. Everything Win-related broke.

**Root cause (debug log):**
```
hook: Win+(non-Space 0x46) → re-injecting Win 0x5B down
hook: Win down (vk=0x5B) → tentatively swallowed   <- our own injection!
```
The build swallowed the Win **keydown** and re-injected it via `SendInput`
when the next key wasn't Space. The injected Win keydown re-entered our own
LL hook and was swallowed again (infinite). The Win key effectively died.

**Fix:** abandoned swallow-Win-keydown entirely. The winning approach touches
only Space and uses a dummy injected key (`VK_F20`), with injected events
recognized via `LLKHF_INJECTED` + `INJECT_MAGIC` `dwExtraInfo` + an
`INJECTING` atomic and passed straight through.

## Bug 7 — Win+Space stops summoning entirely (dummy-key build)

**Symptom:** after switching to the dummy-key approach, no Win+Space press
fired Summon (debug log: `fresh=false` on every press), yet the tray
left-click summon worked fine.

**Root cause:** I added `space_held_now()` (`GetAsyncKeyState(VK_SPACE)`
**high bit**) as an AND precondition for firing. Inside the LL hook callback
for a key's *own* keydown, `GetAsyncKeyState` has not yet recorded that key
as down — so it returns false and short-circuits the AND. This is the
high-bit analogue of Bug 5's low-bit unreliability.

**Fix:** drop `space_held_now()` from the fresh-press check; the time
debounce alone is sufficient and reads no key state.

**Generalized lesson:** inside a `WH_KEYBOARD_LL` callback, do **not** use
`GetAsyncKeyState` to read the state of the *same* key whose event you are
currently processing — neither the low nor the high bit is reliable there.
Read other keys (the Win modifier) fine; for the hooked key itself, infer
state from the event stream or a time debounce.

## Bug 8 — Win+Space takes a screenshot

**Symptom:** every Win+Space summon also triggered the Windows 11 Snipping
Tool / screenshot capture.

**Root cause:** the dummy intervening key was `VK_F24` (0x87), which Windows
11 binds to the Snipping Tool.

**Fix:** switched the dummy key to `VK_F20` (0x83), which no system shell
shortcut, IME action, or accessibility feature reacts to. (Ruled out by Codex
consultation: F24=snip, F23=Copilot, F17=reserved at shutdown, OEM_*=layout
dependent / Win-shortcuts, IME keys out.)

---

## Recurring themes

- **Tracked state across hook calls is dangerous.** Any boolean that depends
  on receiving a matching keyup can get stuck when that keyup is dropped
  (focus changes, UAC, timeouts). Prefer reading live state or a time
  debounce.
- **`GetAsyncKeyState` is unreliable for the key being hooked**, both bits.
  Use it only for *other* keys (the Win modifier).
- **Injected keys re-enter your own hook.** Always recognize your own
  injections and pass them through.
- **The file-backed debug log is essential.** Every bug above was diagnosed
  from event-ordering in the log, not from reasoning. Reproduce, then read
  the timestamps.
