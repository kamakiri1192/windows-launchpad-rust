//! Windows OS integration for the resident launcher: a low-level keyboard
//! hook that summons the launcher with **Win+Space** (and suppresses the
//! system IME switch on that combo), plus a **notification-area (tray) icon**
//! with a context menu offering "Show" / "Quit".
//!
//! Both subsystems require a message pump, so they share a single dedicated
//! thread (`OsIntegrationHandle`) that owns its own `GetMessageW` loop. The
//! thread is cheap while idle (it blocks in `GetMessageW`); the hot key
//! arrives via the LL hook callback, and tray events arrive on the
//! `WM_APP`-range callback message.
//!
//! All UI-thread interaction is one-way: this thread only ever
//! `EventLoopProxy::send_event` into the winit loop and returns. In particular
//! the LL hook callback does *no* work beyond a cheap state read and one
//! `send_event`, because `WH_KEYBOARD_LL` callbacks that block longer than
//! `LowLevelHooksTimeout` get silently removed by Windows.
//!
//! This whole module is gated by `#[cfg(windows)]` on the `mod` declaration in
//! `main.rs`, so we do *not* repeat `#![cfg(windows)]` here (clippy's
//! `duplicated_attributes` would flag the duplicate).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    HBITMAP,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_TYPE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, NOTIFY_ICON_DATA_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, InsertMenuItemW,
    PostMessageW, RegisterClassExW, SetForegroundWindow, SetWindowsHookExW, TrackPopupMenu,
    TranslateMessage, UnhookWindowsHookEx, CS_HREDRAW, CS_VREDRAW, HICON, HWND_MESSAGE, ICONINFO,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MENUITEMINFOW, MENU_ITEM_MASK, MENU_ITEM_STATE,
    MENU_ITEM_TYPE, MSG, TRACK_POPUP_MENU_FLAGS, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_COMMAND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WNDCLASSEXW,
};

use crate::{app_icon, UserEvent};

/// App-private window message used by the shell to deliver tray notifications.
/// Anything in the `WM_APP`..`WM_APP+0x7FFF` range is safe for this.
const WM_TRAYICON: u32 = 0x8000; // WM_APP

/// Menu item ids for the tray popup.
const ID_SHOW: u32 = 1001;
const ID_QUIT: u32 = 1002;

// Virtual key codes. The `windows` crate exposes `VK_*` behind the
// `Win32_UI_Input_KeyboardAndMouse` feature; the constants are stable and
// tiny, so we inline them to avoid adding a feature flag.
const VK_SPACE: u16 = 0x20;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;
/// VK_F20: a key that no Windows shell shortcut reacts to. We inject a quick
/// F20 down/up right after a Win+Space so the OS sees an intervening keypress
/// between Win-down and Win-up — which stops the shell from reading the
/// release as a lone Win tap (the cause of the Start-menu popping).
///
/// Why F20 and not F24: F24 (0x87) triggers the Snipping Tool / screenshot on
/// Windows 11. F20 (0x83) is a defined VK but is not bound to any system
/// shortcut, IME action, or accessibility feature. Other F-keys are also
/// unsafe: F23 is the Copilot key, F17 is reserved at shutdown. See the MS
/// virtual-key-codes table.
const VK_F20: u16 = 0x83;
/// Magic tag written into the dwExtraInfo of every key WE inject via
/// SendInput, so our own LL hook can recognize and pass them through (the
/// injected event would otherwise be re-swallowed, which is what broke the
/// earlier "swallow Win keydown" attempt). Belt to LLKHF_INJECTED's suspenders.
const INJECT_MAGIC: usize = 0x4C50_575F_5350_4143; // "LP_WSPAC"

/// After a consumed Win+Space, swallow the matching Space keyup for this many
/// ms so no orphan Space-up leaks through to the IME switcher. Generous enough
/// to cover focus-change keyup delays (proven to happen in the debug log).
const COMBO_SUPPRESS_MS: u64 = 1500;

// MENUITEMINFOW mask bits (MIIM_*).
const MIIM_STRING: u32 = 0x0000_0040;
const MIIM_ID: u32 = 0x0000_0002;
const MIIM_STATE: u32 = 0x0000_0004;

// MENUITEMINFOW state/type bit values (MFS_*/MFT_*).
const MFS_ENABLED: u32 = 0x0000_0000;
const MFS_DEFAULT: u32 = 0x0000_1000;
const MFT_STRING: u32 = 0x0000_0000;

// NOTIFYICONDATAW flag bits (NIF_*).
const NIF_MESSAGE: u32 = 0x0000_0001;
const NIF_ICON: u32 = 0x0000_0002;
const NIF_TIP: u32 = 0x0000_0004;

// TrackPopupMenu flags (TPM_*).
const TPM_LEFTALIGN: u32 = 0x0000;
const TPM_BOTTOMALIGN: u32 = 0x0020;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;

/// Handle to the OS-integration thread. Dropping it requests the thread to
/// exit, unregisters the hook, and removes the tray icon. Kept alive for the
/// whole process by being stored on `App`.
pub struct OsIntegrationHandle {
    /// Sender used to ask the integration thread to exit.
    quit_tx: Option<mpsc::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for OsIntegrationHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.quit_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

impl OsIntegrationHandle {
    /// Spawn the integration thread. It registers the LL keyboard hook and
    /// the tray icon, then runs a message loop until told to quit.
    pub fn spawn(proxy: winit::event_loop::EventLoopProxy<UserEvent>) -> Self {
        let (quit_tx, quit_rx) = mpsc::channel::<()>();

        let join = thread::Builder::new()
            .name("os-integration".to_string())
            .spawn(move || run_integration_thread(proxy, quit_rx))
            .expect("spawn os-integration thread");

        Self {
            quit_tx: Some(quit_tx),
            join: Some(join),
        }
    }
}

/// The integration thread entry point. Owns the hook + tray icon for its
/// lifetime and pumps messages until `quit_rx` fires.
fn run_integration_thread(
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    quit_rx: mpsc::Receiver<()>,
) {
    // Install the per-thread shared state the hook callback reads. We use a
    // thread-local so the callback (which has no user-data argument for LL
    // hooks) can find the proxy + key-tracking state. Set exactly once on
    // this thread, read only from this thread's hook callbacks.
    HOOK_STATE.with(|cell| {
        *cell.borrow_mut() = Some(HookState {
            proxy,
            last_summon_ms: 0,
            suppress_space_up_until_ms: 0,
        });
    });

    // Install the low-level keyboard hook. LL hooks are effectively
    // thread-affine: the installing thread must pump messages.
    let hook = unsafe {
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("os-integration: SetWindowsHookExW failed: {e}");
                return;
            }
        }
    };

    // Create a message-only window to own the tray icon. A message-only HWND
    // is invisible and doesn't appear in the shell; it's the standard owner
    // for Shell_NotifyIconW callback routing.
    let hwnd = match create_message_only_window() {
        Some(h) => h,
        None => {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
            return;
        }
    };

    // Register the tray icon from the same artwork as the app/window icon.
    let icon = create_app_icon();
    add_tray_icon(hwnd, icon);

    // Message pump. Peek for our quit signal between blocking GetMessageW
    // calls; if the channel fires, post a WM_QUIT to ourselves to break out.
    let mut msg = MSG::default();
    loop {
        if quit_rx.try_recv().is_ok() {
            break;
        }
        // Block until a message arrives (cheap while idle).
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match got.0 {
            0 | -1 => break, // WM_QUIT or error
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }

    // Cleanup: remove tray icon, destroy it, uninstall hook.
    remove_tray_icon(hwnd);
    unsafe {
        if !icon.is_invalid() {
            let _ = DestroyIcon(icon);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    // The message-only HWND is destroyed implicitly on thread exit.
}

/// Shared state between the integration thread and the LL hook callback.
struct HookState {
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    /// Timestamp (ms, from GetTickCount64) of the last Summon we fired. Used
    /// to suppress auto-repeat WITHOUT a tracked key flag: the debug log
    /// proved the hook sometimes never sees the Space keyup (the focus change
    /// on summon appears to drop it), which left a tracked flag stuck and made
    /// the next press look like a repeat. A time debounce doesn't depend on
    /// keyup delivery at all.
    last_summon_ms: u64,
    /// Until when (ms) to swallow the matching Space keyup after a consumed
    /// Win+Space, so no orphan Space-up leaks to the IME switcher. Cleared
    /// once used or after COMBO_SUPPRESS_MS.
    suppress_space_up_until_ms: u64,
}

/// Auto-repeat debounce window in milliseconds. A typical keyboard repeats
/// at ~30Hz (33ms) once the OS repeat kicks in (~500ms after press-down), so
/// 400ms comfortably swallows a held key's auto-repeats while letting a
/// genuine second press (which takes a human >400ms to produce) fire.
const SUMMON_DEBOUNCE_MS: u64 = 400;

thread_local! {
    /// Per-thread slot holding the hook state. Only the integration thread
    /// ever sets or reads it (its callbacks run on that thread), so this is
    /// effectively a thread-local with a single owner.
    static HOOK_STATE: std::cell::RefCell<Option<HookState>> = const { std::cell::RefCell::new(None) };
}

/// Read whether a Win modifier is physically held right now, straight from
/// the keyboard state. We deliberately do NOT track Win down/up ourselves:
/// the debug log on the test machine proved that any such flag gets poisoned
/// across sessions (a Win keyup that the hook never receives leaves the flag
/// stuck), and then the *next* Win+Space is mis-handled, which is exactly
/// the "Start menu opens on the 2nd summon" bug.
fn win_held_now() -> bool {
    // GetAsyncKeyState returns an i16; the high bit (sign bit) is set when
    // the key is currently down. Two calls (LWIN/RWIN) are cheap and safe
    // inside the hook callback; they read a kernel-maintained state, not a
    // syscall into anything that could stall past LowLevelHooksTimeout.
    unsafe { GetAsyncKeyState(VK_LWIN as i32) < 0 || GetAsyncKeyState(VK_RWIN as i32) < 0 }
}

/// Read whether Space is physically held right now (high bit only — the low
/// "pressed since last call" bit is unreliable when read inside the hook
/// callback for the very key being hooked, per MSDN and per our debug log).
fn space_held_now() -> bool {
    unsafe { GetAsyncKeyState(VK_SPACE as i32) < 0 }
}

/// Inject a quick VK_F20 down/up while Win is held, so the OS no longer sees
/// a lone "Win down ... Win up" and therefore doesn't pop the Start menu on a
/// consumed Win+Space. The injected events are tagged with INJECT_MAGIC and
/// are also flagged LLKHF_INJECTED by the system, so our own hook recognizes
/// them and passes them through (this is the fix for the earlier infinite-
/// re-swallow failure).
unsafe fn inject_dummy_key() -> bool {
    let inputs = [dummy_input(false), dummy_input(true)];
    let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    sent as usize == inputs.len()
}

/// Build one VK_F20 INPUT, tagged with INJECT_MAGIC. `up` selects key-up vs
/// key-down.
fn dummy_input(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_TYPE(1), // INPUT_KEYBOARD
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_F20),
                wScan: 0,
                dwFlags: if up {
                    KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0)
                } else {
                    KEYBD_EVENT_FLAGS::default()
                },
                time: 0,
                dwExtraInfo: INJECT_MAGIC,
            },
        },
    }
}

/// `AtomicBool` guard set while we are inside our own `inject_dummy_key`
/// call, as a belt-and-suspenders way to make sure we never recursively
/// swallow our injected F24 even if the LLKHF_INJECTED flag check somehow
/// misses (it won't, but defensive).
static INJECTING: AtomicBool = AtomicBool::new(false);

/// The low-level keyboard hook callback.
///
/// Strategy (informed by Codex consultation + debug-log findings):
///
/// 1. **Never track the Win modifier.** A Win keyup that the hook never
///    receives (UAC / timeout / focus steal) would leave a tracked flag stuck
///    true and break the *next* Win+Space — proven root cause of the earlier
///    "Start menu opens on 2nd summon" bug. Win state is read live via
///    `GetAsyncKeyState`.
///
/// 2. **Pass injected events through.** `SendInput`-injected keys re-enter
///    this hook (flagged `LLKHF_INJECTED`). If we swallowed them we'd get
///    infinite re-swallow (proven failure of the earlier "swallow Win keydown
///    + re-inject" attempt). So any event with `LLKHF_INJECTED` set OR with
///      our `INJECT_MAGIC` dwExtraInfo is passed straight to the next hook.
///
/// 3. **Suppress the Start menu with a dummy key.** When Win+Space fires, we
///    swallow Space AND inject a quick VK_F20 down/up while Win is still
///    held. The OS now sees "Win down, F20, Win up" instead of a bare Win
///    tap, so the shell does NOT open Start. This lets the real Win keyup
///    pass normally (Win+E etc. stay intact) — we don't touch Win at all.
///
/// 4. **Time debounce for fresh-press detection.** A tracked Space latch
///    also gets poisoned by dropped keyups, so we use GetTickCount64 +
///    SUMMON_DEBOUNCE_MS instead.
extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // HC_ACTION == 0 means there's a keyboard event in lparam.
    const HC_ACTION: i32 = 0;
    if code != HC_ACTION {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode as u16;
    let wparam_val = wparam.0 as u32;
    let keydown = wparam_val == WM_KEYDOWN || wparam_val == WM_SYSKEYDOWN;
    let keyup = wparam_val == WM_KEYUP || wparam_val == WM_SYSKEYUP;
    let now_ms = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };

    // --- Rule 2: pass injected events through. ---
    // LLKHF_INJECTED is set by the system on SendInput-produced events; we
    // also tag our own dwExtraInfo with INJECT_MAGIC. Either signal means
    // "not a real keystroke, don't touch it" — this is what stops our own
    // injected F20 from being re-swallowed.
    let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0
        || kb.dwExtraInfo == INJECT_MAGIC
        || INJECTING.load(Ordering::Relaxed);
    if injected {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // --- Suppress the orphan Space keyup belonging to a consumed combo. ---
    if keyup && vk == VK_SPACE {
        let swallow = HOOK_STATE.with(|cell| {
            let mut slot = cell.borrow_mut();
            let Some(state) = slot.as_mut() else {
                return false;
            };
            if now_ms <= state.suppress_space_up_until_ms {
                state.suppress_space_up_until_ms = 0;
                return true;
            }
            false
        });
        if swallow {
            crate::debug_log!("hook: Space keyup swallowed (orphan from combo)");
            return LRESULT(1);
        }
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    if !keydown || vk != VK_SPACE {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let win_held = win_held_now();
    if !win_held {
        // Bare Space — pass through untouched.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // Genuine Win+Space keydown. Decide fire vs. swallow via time debounce.
    // NOTE: we deliberately do NOT consult space_held_now() (GetAsyncKeyState
    // high bit for VK_SPACE) here, even though it sounds right. Inside the LL
    // hook callback for a key's OWN keydown, GetAsyncKeyState has not yet
    // recorded that key as down — so it returns false and makes EVERY press
    // look stale, which is exactly what broke the dummy-key build ("fresh=false"
    // on every press, no Summon ever fired). The time debounce alone is
    // sufficient and reliable.
    let fresh = HOOK_STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        let elapsed = now_ms.saturating_sub(state.last_summon_ms);
        let fresh = elapsed > SUMMON_DEBOUNCE_MS;
        if fresh {
            state.last_summon_ms = now_ms;
            state.suppress_space_up_until_ms = now_ms + COMBO_SUPPRESS_MS;
        }
        fresh
    });

    crate::debug_log!("hook: Win+Space, fresh={}", fresh);

    if fresh {
        crate::debug_log!("hook: Win+Space → Summon (firing) + dummy key");
        // Fire the Summon first, then inject the dummy key so the OS sees an
        // intervening keypress and won't read the Win release as a lone tap.
        let _ = HOOK_STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|s| s.proxy.send_event(UserEvent::Summon))
        });
        INJECTING.store(true, Ordering::Relaxed);
        let injected_ok = unsafe { inject_dummy_key() };
        INJECTING.store(false, Ordering::Relaxed);
        crate::debug_log!("hook: dummy key injected={}", injected_ok);
    } else {
        crate::debug_log!("hook: Win+Space repeat (swallowed, debounce)");
    }

    // Always swallow the Space keydown while Win is held so the IME switcher
    // never sees the combo (auto-repeat included).
    LRESULT(1)
}

/// Send a `UserEvent` to the winit event loop from the tray window proc.
/// Best-effort: a closed loop just means we're shutting down.
fn send_from_tray(ev: UserEvent) {
    let sent = HOOK_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            state.proxy.send_event(ev).is_ok()
        } else {
            false
        }
    });
    crate::debug_log!("send_from_tray: event sent={}", sent);
}

// ----------------------------------------------------------------------------
// Tray window + menu plumbing
// ----------------------------------------------------------------------------

/// Create a message-only window to own the tray icon. Returns its HWND.
/// Uses a NULL hInstance (fine for message-only windows).
fn create_message_only_window() -> Option<HWND> {
    let class_name = w!("LaunchpadTray");

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(tray_wnd_proc),
        lpszClassName: class_name,
        style: CS_HREDRAW | CS_VREDRAW,
        ..Default::default()
    };

    unsafe {
        let _atom = RegisterClassExW(&wc);
        // atom == 0 may mean already registered from a prior run; that's fine,
        // CreateWindowExW will still find the class.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("Launchpad"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        );
        hwnd.ok()
    }
}

/// Window proc for the message-only tray window. Handles the tray callback
/// message (right-click → popup menu) and the resulting WM_COMMAND.
extern "system" fn tray_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_TRAYICON {
        // lparam's low word is the mouse message.
        let mouse = (lparam.0 as usize) & 0xFFFF;
        if mouse == WM_RBUTTONUP as usize {
            show_tray_menu(hwnd);
            return LRESULT(0);
        }
        if mouse == WM_LBUTTONUP as usize {
            // Left click also summons, like the "Show" menu item.
            send_from_tray(UserEvent::Summon);
            return LRESULT(0);
        }
        return LRESULT(0);
    }
    if msg == WM_COMMAND {
        let cmd = (wparam.0 as u32) & 0xFFFF;
        crate::debug_log!("tray: WM_COMMAND cmd={}", cmd);
        match cmd {
            ID_SHOW => {
                crate::debug_log!("tray: → Summon");
                send_from_tray(UserEvent::Summon);
            }
            ID_QUIT => {
                crate::debug_log!("tray: → QuitRequested");
                send_from_tray(UserEvent::QuitRequested);
            }
            _ => {}
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Show the tray context menu at the cursor and dispatch the chosen command.
fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return;
        }
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };

        // Build the menu items. Use wide, NUL-terminated UTF-16 strings.
        let mut show_text: Vec<u16> = "表示".encode_utf16().collect();
        show_text.push(0);
        let mut quit_text: Vec<u16> = "終了".encode_utf16().collect();
        quit_text.push(0);

        let show_item = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MENU_ITEM_MASK(MIIM_STRING | MIIM_ID | MIIM_STATE),
            fType: MENU_ITEM_TYPE(MFT_STRING),
            fState: MENU_ITEM_STATE(MFS_DEFAULT | MFS_ENABLED),
            wID: ID_SHOW,
            dwTypeData: windows::core::PWSTR(show_text.as_mut_ptr()),
            cch: (show_text.len() - 1) as u32,
            ..Default::default()
        };
        let _ = InsertMenuItemW(menu, 0, true, &show_item);

        let quit_item = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MENU_ITEM_MASK(MIIM_STRING | MIIM_ID | MIIM_STATE),
            fType: MENU_ITEM_TYPE(MFT_STRING),
            fState: MENU_ITEM_STATE(MFS_ENABLED),
            wID: ID_QUIT,
            dwTypeData: windows::core::PWSTR(quit_text.as_mut_ptr()),
            cch: (quit_text.len() - 1) as u32,
            ..Default::default()
        };
        let _ = InsertMenuItemW(menu, 1, true, &quit_item);

        // Required for the menu to dismiss on outside click.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TRACK_POPUP_MENU_FLAGS(
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            ),
            cursor.x,
            cursor.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        if chosen.0 != 0 {
            crate::debug_log!("tray: menu chose id={} → posting WM_COMMAND", chosen.0);
            let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(chosen.0 as usize), LPARAM(0));
        } else {
            crate::debug_log!("tray: menu dismissed with no selection");
        }
    }
}

// ----------------------------------------------------------------------------
// Tray icon registration
// ----------------------------------------------------------------------------

/// Register the tray icon with the shell.
fn add_tray_icon(hwnd: HWND, icon: HICON) {
    unsafe {
        let tip: Vec<u16> = "Launchpad".encode_utf16().collect();
        let mut sz_tip = [0u16; 128];
        let cap = sz_tip.len() - 1;
        for (i, &ch) in tip.iter().enumerate().take(cap) {
            sz_tip[i] = ch;
        }

        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NOTIFY_ICON_DATA_FLAGS(NIF_MESSAGE | NIF_ICON | NIF_TIP),
            uCallbackMessage: WM_TRAYICON,
            hIcon: icon,
            szTip: sz_tip,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

/// Remove the tray icon.
fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

// ----------------------------------------------------------------------------
// Runtime HICON for the notification area
// ----------------------------------------------------------------------------

/// Create a small HICON from the bundled app icon artwork. Returns an owned
/// HICON (caller destroys it).
fn create_app_icon() -> HICON {
    let Some(icon) = app_icon::load_rgba(Some(32)) else {
        return HICON(std::ptr::null_mut());
    };
    let w = icon.width as i32;
    let h = icon.height as i32;

    // CreateDIBSection expects BGRA bytes. The bundled asset decodes as RGBA.
    let mut pixels = Vec::with_capacity(icon.rgba.len());
    for px in icon.rgba.chunks_exact(4) {
        pixels.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }

    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        // Color DIB section holding our BGRA pixels.
        let mut ppv: *mut c_void = std::ptr::null_mut();
        let hbm_color: HBITMAP =
            match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut ppv, None, 0) {
                Ok(bm) => {
                    if !ppv.is_null() {
                        std::ptr::copy_nonoverlapping(
                            pixels.as_ptr(),
                            ppv as *mut u8,
                            pixels.len(),
                        );
                    }
                    bm
                }
                Err(_) => return HICON(std::ptr::null_mut()),
            };

        // AND mask: all zeros (we use alpha instead).
        let and_mask: Vec<u8> = vec![0; (((w + 7) / 8) * h) as usize];
        let hbm_mask: HBITMAP = CreateBitmap(w, h, 1, 1, Some(and_mask.as_ptr() as *const c_void));

        let ii = ICONINFO {
            fIcon: windows::Win32::Foundation::TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };

        let icon = CreateIconIndirect(&ii).unwrap_or_default();

        // The bitmaps can be deleted after CreateIconIndirect copies them.
        if !hbm_color.is_invalid() {
            let _ = DeleteObject(hbm_color.into());
        }
        if !hbm_mask.is_invalid() {
            let _ = DeleteObject(hbm_mask.into());
        }
        icon
    }
}
