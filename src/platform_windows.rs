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
use std::sync::mpsc;
use std::thread;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    HBITMAP,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, NOTIFY_ICON_DATA_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, InsertMenuItemW,
    PostMessageW, RegisterClassExW, SetForegroundWindow, SetWindowsHookExW, TrackPopupMenu,
    TranslateMessage, UnhookWindowsHookEx, CS_HREDRAW, CS_VREDRAW, HICON, HWND_MESSAGE, ICONINFO,
    KBDLLHOOKSTRUCT, MENUITEMINFOW, MENU_ITEM_MASK, MENU_ITEM_STATE, MENU_ITEM_TYPE, MSG,
    TRACK_POPUP_MENU_FLAGS, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONUP, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW,
};

use crate::UserEvent;

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
            win_down: false,
            combo_consumed: false,
            space_latched: false,
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

    // Register the tray icon. The icon bitmap is procedurally generated (a
    // simple rounded square) so we don't need to ship a .ico resource.
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
    /// Tracks the Win modifier's down/up transitions. This is NOT used to
    /// decide whether Space is part of a Win+Space combo (that's polled live
    /// via `GetAsyncKeyState` so a dropped keyup can never poison it). It only
    /// scopes the `combo_consumed` flag to a single Win press.
    win_down: bool,
    /// True once we've consumed a Win+Space on the *current* Win press. While
    /// set, we also swallow the matching Win keyup so Windows doesn't see a
    /// bare "Win down ... Win up" and open the Start menu (which is what was
    /// happening: we swallowed Space but let Win through, so the OS read it as
    /// a lone Win tap). Reset on the next Win keydown.
    combo_consumed: bool,
    /// Suppresses Space auto-repeat: once Win+Space fires a Summon, the next
    /// Space must be a genuine release+repress before another Summon fires.
    space_latched: bool,
}

thread_local! {
    /// Per-thread slot holding the hook state. Only the integration thread
    /// ever sets or reads it (its callbacks run on that thread), so this is
    /// effectively a thread-local with a single owner.
    static HOOK_STATE: std::cell::RefCell<Option<HookState>> = const { std::cell::RefCell::new(None) };
}

/// Read whether a Win modifier is physically held right now, straight from
/// the keyboard state. We do NOT track this ourselves (a missed keyup —
/// UAC, a timeout-induced dropped hook call, a full-screen app stealing
/// focus — would otherwise leave our flag stuck `true` and let bare Space
/// trigger Summon forever).
fn win_held_now() -> bool {
    // GetAsyncKeyState returns an i16; the high bit (sign bit) is set when
    // the key is currently down. Two calls (LWIN/RWIN) are cheap and safe
    // inside the hook callback; they read a kernel-maintained state, not a
    // syscall into anything that could stall past LowLevelHooksTimeout.
    unsafe { GetAsyncKeyState(VK_LWIN as i32) < 0 || GetAsyncKeyState(VK_RWIN as i32) < 0 }
}

/// The low-level keyboard hook callback.
///
/// Safety contract: must return quickly. Does only: read the
/// KBDLLHOOKSTRUCT, update a few flags on the shared state, and call
/// `send_event`. On Win+Space it returns a non-zero LRESULT to *swallow* the
/// keystroke (this is what suppresses the system IME switch); otherwise it
/// chains to the next hook.
///
/// Two tricky bits:
///
/// - **The "Space alone triggers Summon" bug.** We must NOT trust a tracked
///   `win_down` flag to decide whether a Space is part of a Win+Space combo,
///   because a dropped Win keyup (UAC, hook timeout, fullscreen focus steal,
///   sticky keys, injected events) would leave it stuck and every subsequent
///   bare Space would fire Summon. So the combo check polls the live hardware
///   state via `GetAsyncKeyState`. The tracked flags below are only used for
///   *swallowing* — deciding what the OS is allowed to see.
///
/// - **The "Start menu opens after summon" bug.** If we swallow only Space,
///   the OS sees a bare "Win down ... Win up" and opens the Start menu. So
///   once a Win+Space is consumed, we also swallow the *matching Win keyup*.
///   The `combo_consumed` flag is scoped to a single Win press and reset on
///   the next Win keydown, so a dropped keyup can at worst drop one Start-menu
///   suppress — never poison subsequent presses.
extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // HC_ACTION == 0 means there's a keyboard event in lparam.
    const HC_ACTION: i32 = 0;
    if code != HC_ACTION {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let wparam_val = wparam.0 as u32;
    let keydown = wparam_val == WM_KEYDOWN || wparam_val == WM_SYSKEYDOWN;
    let keyup = wparam_val == WM_KEYUP || wparam_val == WM_SYSKEYUP;

    let action = HOOK_STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return HookAction::Chain;
        };
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = kb.vkCode as u16;
        let is_win = vk == VK_LWIN || vk == VK_RWIN;

        // Debug trace: log every event we inspect, so we can see the actual
        // event order/flags on the failing machine. Remove after fixing.
        eprintln!(
            "hook: vk=0x{:02X} win={} down={} up={} win_down={} consumed={} latched={}",
            vk, is_win, keydown, keyup, state.win_down, state.combo_consumed, state.space_latched,
        );

        // Win modifier transitions: track scope + swallow its keyup when we
        // consumed a combo during this press.
        if is_win && keydown {
            state.win_down = true;
            state.combo_consumed = false;
            return HookAction::Chain;
        }
        if is_win && keyup {
            state.win_down = false;
            let consumed = state.combo_consumed;
            state.combo_consumed = false;
            if consumed {
                // Swallow the Win keyup so the OS doesn't read a lone Win tap
                // (which opens the Start menu).
                eprintln!("hook: swallowing Win keyup (combo was consumed)");
                return HookAction::Swallow;
            }
            return HookAction::Chain;
        }

        if vk != VK_SPACE {
            return HookAction::Chain;
        }

        if keydown {
            if win_held_now() {
                // Genuine Win+Space. Fire once per press (latched), swallow
                // the Space so the IME switcher never sees the combo, and
                // remember we consumed it so the matching Win keyup is also
                // swallowed (prevents the Start-menu-open bug).
                if !state.space_latched {
                    state.space_latched = true;
                    state.combo_consumed = true;
                    eprintln!("hook: Win+Space → Summon");
                    let _ = state.proxy.send_event(UserEvent::Summon);
                }
                return HookAction::Swallow;
            }
            // Space without Win — pass through untouched.
            state.space_latched = false;
            return HookAction::Chain;
        }

        if keyup {
            // Clear the auto-repeat latch on Space release so the next press
            // can fire again. Swallow the keyup too if Win was held (the
            // combo's keyup belongs to us), otherwise let it through.
            let held = win_held_now();
            let was_latched = state.space_latched;
            state.space_latched = false;
            if was_latched && held {
                return HookAction::Swallow;
            }
            return HookAction::Chain;
        }

        HookAction::Chain
    });

    match action {
        HookAction::Swallow => LRESULT(1),
        HookAction::Chain => unsafe { CallNextHookEx(None, code, wparam, lparam) },
    }
}

enum HookAction {
    Swallow,
    Chain,
}

/// Send a `UserEvent` to the winit event loop from the tray window proc.
/// Best-effort: a closed loop just means we're shutting down.
fn send_from_tray(ev: UserEvent) {
    HOOK_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            let _ = state.proxy.send_event(ev);
        }
    });
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
        match cmd {
            ID_SHOW => send_from_tray(UserEvent::Summon),
            ID_QUIT => send_from_tray(UserEvent::QuitRequested),
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
            let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(chosen.0 as usize), LPARAM(0));
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
// Procedurally generated HICON (no .ico resource needed)
// ----------------------------------------------------------------------------

/// Create a small (16x16) HICON with a simple procedurally drawn glyph. Returns
/// an owned HICON (caller destroys it).
fn create_app_icon() -> HICON {
    const W: i32 = 16;
    const H: i32 = 16;
    // BGRA 32-bpp, top-down DIB.
    let mut pixels: Vec<u8> = vec![0; (W * H * 4) as usize];
    // Draw a translucent rounded square with a calm blue fill.
    for y in 0..H {
        for x in 0..W {
            let dx = (x - W / 2).abs();
            let dy = (y - H / 2).abs();
            let inside = (1..W - 1).contains(&x) && (1..H - 1).contains(&y);
            // Round the corners.
            let corner_ok = !(dx > 5 && dy > 5);
            if inside && corner_ok {
                let idx = ((y * W + x) * 4) as usize;
                pixels[idx] = 0xC0; // B
                pixels[idx + 1] = 0x7A; // G
                pixels[idx + 2] = 0x3A; // R
                pixels[idx + 3] = 0xE0; // A
            }
        }
    }

    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: W,
                biHeight: -H, // top-down
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
        let and_mask: Vec<u8> = vec![0; (((W + 7) / 8) * H) as usize];
        let hbm_mask: HBITMAP = CreateBitmap(W, H, 1, 1, Some(and_mask.as_ptr() as *const c_void));

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
