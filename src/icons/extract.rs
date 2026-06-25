//! Enumerate Start Menu `.lnk` files and extract each shortcut's icon into
//! a raw RGBA bitmap.
//!
//! Strategy:
//!   - Resolve the Start Menu known folders (per-user + all-users) via
//!     `SHGetKnownFolderPath`.
//!   - Walk each folder for `*.lnk` files.
//!   - For each `.lnk`, resolve the **target** path via `IShellLinkW` +
//!     `IPersistFile` (so we bypass the shortcut-arrow overlay), then pull
//!     the largest available icon from the shell image list — preferring
//!     `SHIL_JUMBO` (256px), falling back to `SHIL_EXTRALARGE` (48px) and
//!     finally `SHGetFileInfo(SHGFI_LARGEICON)`.
//!   - Convert the `HICON` to 32-bit BGRA via `GetIconInfo` + `GetDIBits`,
//!     then swizzle to RGBA for the atlas.
//!
//! All Win32 handles (`HICON`, `HBITMAP`, `HDC`) are wrapped in RAII guards so
//! a panic or early return can't leak GDI objects.

use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HANDLE, SIZE};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, FindNextFileW, FILE_ATTRIBUTE_DIRECTORY, WIN32_FIND_DATAW,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Controls::IImageList;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartMenu, FOLDERID_StartMenu, ILFree, IShellItemImageFactory, IShellLinkW,
    SHCreateItemFromIDList, SHGetFileInfoW, SHGetImageList, SHGetKnownFolderPath, ShellLink,
    KNOWN_FOLDER_FLAG, SHFILEINFOW, SHGFI_FLAGS, SHGFI_LARGEICON, SHGFI_SYSICONINDEX,
    SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY, SIIGBF_SCALEUP,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use super::DecodedIcon;

/// One enumerated shortcut: its display name (derived from the file stem) and
/// the `.lnk` path on disk.
#[derive(Debug, Clone)]
pub struct Shortcut {
    pub name: String,
    pub path: PathBuf,
}

/// Enumerate `.lnk` files under both the per-user and all-users Start Menus.
///
/// Returns the combined list, duplicates (same file in both folders are rare)
/// kept in encounter order. Errors are logged and skipped so a single
/// unreadable folder can't blank the whole grid.
pub fn enumerate_start_menu() -> Vec<Shortcut> {
    let mut out = Vec::new();
    for folder in [FOLDERID_StartMenu, FOLDERID_CommonStartMenu] {
        if let Some(path) = known_folder_path(folder) {
            collect_lnks(&path, &mut out);
        }
    }
    out
}

/// Extract the best-available icon for a single `.lnk` file as an RGBA bitmap.
///
/// Resolves the shortcut's **target** (so the shortcut-arrow overlay doesn't
/// appear), then pulls the largest icon the shell can provide for that target
/// — preferring 256px jumbo, then 48px extra-large, then 32px large.
///
/// Returns `None` on any failure (missing icon, GDI error); the caller simply
/// drops that tile back to the fallback color.
pub fn extract_icon_from_lnk(lnk: &Path) -> Option<DecodedIcon> {
    // A .lnk can carry its icon in two places:
    //   1. An explicit IconLocation (Discord, Chrome, Slack — Electron apps
    //      whose launcher exe has a generic icon, with the real icon pointed
    //      at by the shortcut's own IconLocation field).
    //   2. The target executable's own icon (most native apps).
    //
    // We query the .lnk FIRST for the system icon index: when the shortcut has
    // an explicit IconLocation, the index reflects that icon; otherwise it
    // reflects the target's icon. Either way we then pull it from the jumbo
    // image list for maximum resolution. Only if the .lnk yields a generic
    // icon do we fall back to resolving the target explicitly.
    //
    // NOTE: this strategy is the carefully-tuned one from main (PR #5/#8). It
    // is intentionally NOT replaced by IShellItemImageFactory-as-primary-path,
    // which produced regressions (Blender and other apps rendered blank). The
    // factory path remains available only via the ignored diagnostic probe.

    let link = load_shell_link(lnk);
    let resolved_target = link.as_ref().and_then(resolve_link_target);

    // (1) Index the .lnk itself and pull the best image-list icon.
    if let Some(h) = get_path_hicon(lnk) {
        let _guard = IconGuard(h);
        if let Some(icon) = hicon_to_rgba(h) {
            if is_mostly_white_icon(&icon) {
                if let Some(target) = find_exe_by_shortcut_name(lnk) {
                    if let Some(h) = get_path_hicon(&target) {
                        let _guard = IconGuard(h);
                        if let Some(exe_icon) = hicon_to_rgba(h) {
                            if !is_mostly_white_icon(&exe_icon) {
                                return Some(exe_icon);
                            }
                        }
                    }
                }

                if let Some(target) = resolved_target.as_deref() {
                    if let Some(h) = get_path_hicon(target) {
                        let _guard = IconGuard(h);
                        if let Some(target_icon) = hicon_to_rgba(h) {
                            return Some(target_icon);
                        }
                    }
                }
            }
            return Some(icon);
        }
    }

    // (2) Resolve the target and index that instead.
    let hicon = get_path_hicon(resolved_target.as_deref().unwrap_or(lnk))?;
    let _guard = IconGuard(hicon);
    hicon_to_rgba(hicon)
}

// ---- shortcut target resolution ----------------------------------------

/// Resolve the target path of a `.lnk` via `IShellLinkW` + `IPersistFile`.
///
/// Returns `None` if the shortcut can't be parsed or has no target. The path
/// has environment variables (`%windir%`, `%SystemRoot%`, …) expanded so the
/// shell can actually locate the file for icon lookup.
fn load_shell_link(lnk: &Path) -> Option<IShellLinkW> {
    let wide = path_to_wide(lnk)?;
    // CoCreateInstance the ShellLink object, then Load the .lnk via IPersistFile.
    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let persist: IPersistFile = link.cast().ok()?;
    // SAFETY: wide is NUL-terminated; STGM_READ = 0.
    unsafe { persist.Load(PCWSTR(wide.as_ptr()), windows::Win32::System::Com::STGM(0)) }.ok()?;
    Some(link)
}

fn resolve_link_target(link: &IShellLinkW) -> Option<PathBuf> {
    // Read the target path. SLGP_RAWPATH = 4 — resolve without UI/hwnd.
    let mut buf = [0u16; 260];
    let mut fd = WIN32_FIND_DATAW::default();
    // SAFETY: buf is a writable 260-wide buffer; fd outlives the call.
    unsafe { link.GetPath(&mut buf, &mut fd, 0x0004) }.ok()?;
    let raw = wide_to_string(&buf);
    if raw.is_empty() {
        return None;
    }
    // IShellLinkW returns the path with %SystemRoot% etc. unexpanded; expand it
    // so SHGetFileInfo can find the real file (otherwise it falls back to a
    // generic exe icon).
    let expanded = expand_env(&raw);
    Some(PathBuf::from(expanded))
}

/// Resolved `.lnk` metadata used for cache keying and snapshot diffing.
///
/// All fields are cheap, owned strings. `target_path` / `icon_location` are
/// environment-expanded so two equivalent spellings compare equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LnkMetadata {
    /// Resolved target path (expanded), or `""` if unresolvable.
    pub target_path: String,
    /// Shell IconLocation field (expanded), or `""` if the icon lives in the
    /// target exe.
    pub icon_location: String,
    /// Icon index inside `icon_location` (or the target when no location set).
    pub icon_index: i32,
}

/// Load a `.lnk` and pull its target path + icon location/index.
///
/// This is the cache-key side of icon extraction: the *bytes* come from
/// [`extract_icon_from_lnk`], but these metadata drive whether a cached icon is
/// still valid. Kept separate so a cache probe can run without paying for GDI.
pub fn resolve_lnk_metadata(lnk: &Path) -> Option<LnkMetadata> {
    let link = load_shell_link(lnk)?;

    let target_path = resolve_link_target(&link)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    // IconLocation: wide buffer + out param for the index.
    let mut icon_buf = [0u16; 260];
    let mut icon_index_i32: i32 = 0;
    // SAFETY: icon_buf is a 260-wide writable buffer.
    let icon_location = unsafe { link.GetIconLocation(&mut icon_buf, &mut icon_index_i32) }
        .ok()
        .map(|_| wide_to_string(&icon_buf))
        .unwrap_or_default();

    Some(LnkMetadata {
        target_path,
        icon_location: if icon_location.is_empty() {
            icon_location
        } else {
            expand_env(&icon_location)
        },
        icon_index: icon_index_i32,
    })
}

/// Last-modified time of a file as a `u64` Windows file-time, or `0` if it
/// can't be read. Used purely for equality comparison in cache invalidation.
pub fn file_mtime(path: &Path) -> u64 {
    use windows::Win32::Storage::FileSystem::{GetFileAttributesExW, GET_FILEEX_INFO_LEVELS};

    let Some(wide) = path_to_wide(path) else {
        return 0;
    };
    let mut data = windows::Win32::Storage::FileSystem::WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: wide is NUL-terminated; data outlives the call.
    let ok = unsafe {
        GetFileAttributesExW(
            PCWSTR(wide.as_ptr()),
            GET_FILEEX_INFO_LEVELS(0),
            &mut data as *mut _ as *mut core::ffi::c_void,
        )
    }
    .is_ok();
    if !ok {
        return 0;
    }
    let ft = data.ftLastWriteTime;
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

fn find_exe_by_shortcut_name(lnk: &Path) -> Option<PathBuf> {
    let stem = lnk.file_stem()?.to_string_lossy();
    let exe_name = normalized_exe_name(&stem)?;
    let dir_name = exe_name.strip_suffix(".exe").unwrap_or(&exe_name);

    exe_name_candidates(dir_name, &exe_name)
        .into_iter()
        .find(|path| path.is_file())
}

fn is_mostly_white_icon(icon: &DecodedIcon) -> bool {
    icon_white_ratio_exceeds(icon, 5, 4)
}

fn icon_white_ratio_exceeds(icon: &DecodedIcon, numerator: usize, denominator: usize) -> bool {
    let mut opaque = 0usize;
    let mut whiteish = 0usize;

    for px in icon.rgba.chunks_exact(4) {
        let [r, g, b, a] = [px[0], px[1], px[2], px[3]];
        if a > 10 {
            opaque += 1;
            if r > 220 && g > 220 && b > 220 {
                whiteish += 1;
            }
        }
    }

    opaque > 1000 && whiteish * numerator > opaque * denominator
}

fn normalized_exe_name(name: &str) -> Option<String> {
    let compact: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if compact.is_empty() {
        None
    } else {
        Some(format!("{compact}.exe"))
    }
}

fn exe_name_candidates(dir_name: &str, exe_name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        paths.push(local.join("Microsoft").join(dir_name).join(exe_name));
        paths.push(local.join("Programs").join(dir_name).join(exe_name));
        paths.push(local.join(exe_name));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let program_files = PathBuf::from(program_files);
        paths.push(program_files.join(dir_name).join(exe_name));
        paths.push(program_files.join(exe_name));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        let program_files_x86 = PathBuf::from(program_files_x86);
        paths.push(program_files_x86.join(dir_name).join(exe_name));
        paths.push(program_files_x86.join(exe_name));
    }
    paths
}

/// Expand `%VAR%` references in `s` via `ExpandEnvironmentStringsW`.
fn expand_env(s: &str) -> String {
    let src: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let mut dst = vec![0u16; 1024];
    // SAFETY: src is NUL-terminated; dst is a writable 1024-wide buffer.
    let len = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut dst)) };
    if len == 0 {
        return s.to_string();
    }
    // `len` includes the terminating NUL.
    wide_to_string(&dst[..(len as usize).min(dst.len())])
}

// ---- icon acquisition --------------------------------------------------

/// Get the largest available `HICON` for a path (a .lnk, exe, or any file),
/// with no shortcut overlay. Resolves the system icon index for the path, then
/// pulls it from the shell image list — preferring jumbo(256) → extra-large(48)
/// → large(32).
///
/// When `path` is a `.lnk`, the index reflects the shortcut's explicit
/// IconLocation if set (so Electron apps like Discord/Chrome get their real
/// icon), otherwise the target's icon.
fn get_path_hicon(path: &Path) -> Option<HICON> {
    let wide = path_to_wide(path)?;

    // Get the system icon index for this file, then pull the icon from the
    // shell image list at the best available size.
    let index = file_icon_index(&wide)?;
    get_hicon_from_system_image_list(index)
}

#[allow(dead_code)] // used by the ignored icon strategy probe test
fn get_link_pidl_image(link: &IShellLinkW) -> Option<DecodedIcon> {
    // SAFETY: GetIDList returns a PIDL allocated by the shell; PidlGuard frees
    // it via ILFree before returning.
    let pidl = unsafe { link.GetIDList() }.ok()?;
    if pidl.is_null() {
        return None;
    }
    let _guard = PidlGuard(pidl);
    let factory: IShellItemImageFactory = unsafe { SHCreateItemFromIDList(pidl) }.ok()?;
    let flags = SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK | SIIGBF_SCALEUP;
    let bitmap = unsafe { factory.GetImage(SIZE { cx: 256, cy: 256 }, flags) }.ok()?;
    let _bitmap_guard = BitmapGuard(bitmap);
    hbitmap_to_rgba(bitmap)
}

fn get_hicon_from_system_image_list(index: i32) -> Option<HICON> {
    // Try each image-list size, best first. SHGetImageList returns the shared
    // system image list; IImageList::GetIcon hands us a copy we must destroy.
    for size in [SHIL_JUMBO, SHIL_EXTRALARGE, SHIL_LARGE] {
        if let Ok(img_list) = unsafe { SHGetImageList::<IImageList>(size as i32) } {
            // ILD_TRANSPARENT = 1 — no blend, just the icon.
            if let Ok(hicon) = unsafe { img_list.GetIcon(index, 1) } {
                if !hicon.is_invalid() {
                    return Some(hicon);
                }
            }
        }
    }
    None
}

/// Get the system image-list icon index for a path.
///
/// Queries the actual file on disk (no `SHGFI_USEFILEATTRIBUTES`): this is
/// critical for `.lnk` files, because only by reading the real shortcut does
/// the shell resolve its explicit IconLocation (Electron apps like Discord and
/// Chrome store their real icon there, not in the launcher exe). The returned
/// index has no shortcut overlay because it points at the IconLocation's icon,
/// not the .lnk itself.
fn file_icon_index(path: &[u16]) -> Option<i32> {
    let mut info = SHFILEINFOW::default();
    let flags = SHGFI_FLAGS(SHGFI_SYSICONINDEX.0 | SHGFI_LARGEICON.0);
    // SAFETY: path is NUL-terminated; info zero-initialized & outlives the call.
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(path.as_ptr()),
            Default::default(),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if result == 0 {
        return None;
    }
    Some(info.iIcon)
}

/// Shell image-list size constants (re-exported from the header as raw u32).
const SHIL_LARGE: u32 = 0;
const SHIL_EXTRALARGE: u32 = 2;
const SHIL_JUMBO: u32 = 4;

/// Convert an `HICON` to a straight-RGBA `DecodedIcon`.
///
/// Uses `GetIconInfo` to reach the color `HBITMAP`, queries its dimensions via
/// `GetObjectW`, then reads pixels as 32-bit BGRA via a single `GetDIBits`
/// call. The result is swizzled to RGBA and alpha is un-premultiplied so the
/// atlas stores straight alpha (the icon shader re-premultiplies on sample).
fn hicon_to_rgba(hicon: HICON) -> Option<DecodedIcon> {
    let mut ii = ICONINFO::default();
    // SAFETY: hicon is valid for the lifetime of this function.
    let ok = unsafe { GetIconInfo(hicon, &mut ii) }.is_ok();
    if !ok {
        return None;
    }
    // GetIconInfo gives us ownership of hbmColor and hbmMask — we must free them.
    let _color_guard = BitmapGuard(ii.hbmColor);
    let _mask_guard = BitmapGuard(ii.hbmMask);

    let color = ii.hbmColor;
    if color.is_invalid() {
        // Monochrome icon (no color bitmap) — not worth handling for v1.
        return None;
    }

    hbitmap_to_rgba(color)
}

fn hbitmap_to_rgba(bitmap: HBITMAP) -> Option<DecodedIcon> {
    // Query the bitmap dimensions with GetObjectW. This is more reliable than
    // using GetDIBits' "query" pass (which can return 0 for some DIB sections).
    let mut bmp = BITMAP::default();
    let bytes = unsafe {
        GetObjectW(
            bitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut core::ffi::c_void),
        )
    };
    if bytes == 0 || bmp.bmWidth <= 0 || bmp.bmHeight <= 0 {
        return None;
    }
    let w = bmp.bmWidth as u32;
    let h = bmp.bmHeight as u32;

    // Use the screen DC (more compatible with premultiplied 32bpp icon bitmaps
    // than a memory DC created via CreateCompatibleDC, which can default to a
    // different bit depth on some systems).
    let screen_dc = unsafe { GetDC(None) };
    if screen_dc.is_invalid() {
        return None;
    }
    let dc_guard = ScreenDcGuard(screen_dc);

    // One-pass read: bottom-up (positive height) 32bpp BI_RGB. We flip rows
    // afterwards so the atlas stores top-down RGBA.
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: h as i32, // positive = bottom-up
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: Default::default(),
    };

    let buf_len = (w * h) as usize * 4;
    let mut pixels = vec![0u8; buf_len];
    // SAFETY: buffer sized for w*h*4 bytes; header is a valid 32bpp bottom-up DIB.
    let copied = unsafe {
        GetDIBits(
            dc_guard.0,
            bitmap,
            0,
            h,
            Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
            &bi as *const BITMAPINFO as *mut BITMAPINFO,
            DIB_RGB_COLORS,
        )
    };
    if copied == 0 {
        return None;
    }

    // Bottom-up → top-down: reverse row order in place.
    flip_rows(&mut pixels, w, h);

    // BGRA → RGBA, and straighten premultiplied alpha so the atlas stores
    // straight alpha (the icon shader will re-premultiply on sample).
    for chunk in pixels.chunks_exact_mut(4) {
        let (b, g, r, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        if a == 0 {
            chunk.copy_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            chunk.copy_from_slice(&[r, g, b, a]);
        } else {
            // Un-premultiply, guarding against divide-by-zero.
            let af = a as u32;
            chunk[0] = ((r as u32 * 255 + af / 2) / af).min(255) as u8;
            chunk[1] = ((g as u32 * 255 + af / 2) / af).min(255) as u8;
            chunk[2] = ((b as u32 * 255 + af / 2) / af).min(255) as u8;
            chunk[3] = a;
        }
    }

    Some(DecodedIcon { rgba: pixels, w, h })
}

/// Reverse the row order of a tightly-packed bottom-up bitmap in place.
fn flip_rows(rgba: &mut [u8], w: u32, h: u32) {
    let stride = w as usize * 4;
    let h = h as usize;
    for y in 0..(h / 2) {
        let top = y * stride;
        let bot = (h - 1 - y) * stride;
        // Swap one row at a time via a temporary copy.
        let (head, tail) = rgba.split_at_mut(bot);
        head[top..top + stride].swap_with_slice(&mut tail[..stride]);
    }
}

// ---- known folders & filesystem walk -----------------------------------

/// Resolve a `KNOWNFOLDERID` GUID to a filesystem path. Returns `None` if the
/// folder isn't present (e.g. no all-users Start Menu on some SKUs).
fn known_folder_path(folder: windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: folder is a well-known GUID; htoken=None uses the current user.
    let pwstr = unsafe { SHGetKnownFolderPath(&folder, KNOWN_FOLDER_FLAG(0), None) }.ok()?;
    let wide = unsafe { pwstr.to_string() }.ok()?;
    Some(PathBuf::from(wide))
}

/// Recursively collect `*.lnk` files under `root` into `out`.
fn collect_lnks(root: &Path, out: &mut Vec<Shortcut>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let pattern = dir.join("*");
        let Some(pattern_w) = path_to_wide(&pattern) else {
            continue;
        };
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: pattern_w is NUL-terminated; data outlives the call.
        let handle = unsafe { FindFirstFileW(PCWSTR(pattern_w.as_ptr()), &mut data) };
        let handle = match handle {
            Ok(h) if !h.is_invalid() => h,
            _ => continue,
        };
        let _h_guard = FindGuard(handle);
        loop {
            if !is_dots(&data.cFileName) {
                let name = wide_to_string(&data.cFileName);
                let full = dir.join(&name);
                let is_dir = (data.dwFileAttributes & FILE_ATTRIBUTE_ATTRIBUTES_MASK)
                    == FILE_ATTRIBUTE_DIRECTORY.0;
                if is_dir {
                    stack.push(full);
                } else if name.to_ascii_lowercase().ends_with(".lnk") {
                    let stem = full
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&name)
                        .to_string();
                    out.push(Shortcut {
                        name: stem,
                        path: full,
                    });
                }
            }
            // SAFETY: handle valid; data is a *mut that outlives the call.
            if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
                break;
            }
        }
    }
}

/// `dwFileAttributes` mask for the directory bit, redefined locally because
/// the crate's `FILE_ATTRIBUTE_DIRECTORY` constant route is verbose.
const FILE_ATTRIBUTE_ATTRIBUTES_MASK: u32 = FILE_ATTRIBUTE_DIRECTORY.0;

fn is_dots(name: &[u16]) -> bool {
    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    n == 1 && name[0] == b'.' as u16 || n == 2 && name[0] == b'.' as u16 && name[1] == b'.' as u16
}

/// Encode a `Path` as a UTF-16 buffer terminated with a NUL. Returns `None`
/// if the path contains characters that can't be represented as UTF-16 (rare;
/// all UTF-8 is representable, so this is mostly a guard).
fn path_to_wide(path: &Path) -> Option<Vec<u16>> {
    let s = path.to_str()?;
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    Some(v)
}

/// Decode a UTF-16 (possibly NUL-terminated) buffer into a `String`.
fn wide_to_string(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

// ---- RAII guards -------------------------------------------------------

/// Calls `DestroyIcon` on drop. Never dereferenced.
struct IconGuard(windows::Win32::UI::WindowsAndMessaging::HICON);
impl Drop for IconGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe { _ = DestroyIcon(self.0) };
        }
    }
}

/// Calls `DeleteObject` on drop. Never dereferenced.
struct BitmapGuard(HBITMAP);
impl Drop for BitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe { _ = DeleteObject(self.0.into()) };
        }
    }
}

#[allow(dead_code)] // used by the ignored icon strategy probe test
/// Owns a shell PIDL and frees it on drop.
struct PidlGuard(*mut ITEMIDLIST);
impl Drop for PidlGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ILFree(Some(self.0)) };
        }
    }
}

/// Owns a screen DC and releases it on drop.
struct ScreenDcGuard(HDC);
impl Drop for ScreenDcGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe { _ = ReleaseDC(None, self.0) };
        }
    }
}

/// Owns a find handle and closes it on drop.
struct FindGuard(HANDLE);
impl Drop for FindGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe { _ = FindClose(self.0) };
        }
    }
}

// ---- COM scope ---------------------------------------------------------

/// RAII guard that initializes COM on creation and uninitializes on drop.
///
/// Shell APIs (SHGetFileInfo etc.) technically work without explicit COM init,
/// but `SHGetKnownFolderPath` requires it. We initialize STA to match the
/// single-threaded UI context; nested init returns RPC_E_CHANGED_MODE which we
/// treat as "already initialized" and ignore.
pub struct ComScope;
impl ComScope {
    /// Initialize COM for the current thread. The returned guard uninitializes
    /// on drop; if you don't need teardown, the `Ok(())` branch is harmless to
    /// drop immediately (we hold it explicitly to keep the scope explicit).
    pub fn new() -> Self {
        let r = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // S_OK (0) or S_FALSE (already init) are fine; RPC_E_CHANGED_MODE means
        // the thread is already in MTA — we leave it as-is and continue.
        if r.is_err() && r != windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            eprintln!("CoInitializeEx failed: {r:?}");
        }
        Self
    }
}
impl Drop for ComScope {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_wide_is_nul_terminated() {
        let w = path_to_wide(Path::new("C:\\test.lnk")).unwrap();
        assert_eq!(w.last(), Some(&0u16));
        // No interior NULs.
        assert!(!w[..w.len() - 1].contains(&0));
    }

    #[test]
    fn wide_to_string_strips_trailing_nul() {
        let s = wide_to_string(&[b'H' as u16, b'i' as u16, 0, 99]);
        assert_eq!(s, "Hi");
    }

    #[test]
    fn is_dots_recognizes_special_names() {
        assert!(is_dots(&[b'.' as u16, 0]));
        assert!(is_dots(&[b'.' as u16, b'.' as u16, 0]));
        assert!(!is_dots(&[b'a' as u16, 0]));
    }

    #[test]
    fn expand_env_passes_through_plain_paths() {
        // No %VAR% → returned verbatim.
        let s = expand_env("C:\\Program Files\\App\\app.exe");
        assert_eq!(s, "C:\\Program Files\\App\\app.exe");
    }

    #[test]
    fn expand_env_expands_variables() {
        // %SystemRoot% must resolve to something non-empty and no longer
        // contain the literal "%SystemRoot%".
        let s = expand_env("%SystemRoot%\\system32\\cmd.exe");
        assert!(!s.is_empty());
        assert!(!s.contains("%SystemRoot%"));
        assert!(s.ends_with("\\system32\\cmd.exe"));
    }

    #[test]
    fn flip_rows_reverses_row_order() {
        // 2px wide × 4 rows, 1 byte/pixel/channel for clarity (stride=8).
        // Rows labeled 0,1,2,3 by their first byte.
        let mut rgba: Vec<u8> = vec![
            0, 0, 0, 0, 0, 0, 0, 0, // row 0
            1, 1, 1, 1, 1, 1, 1, 1, // row 1
            2, 2, 2, 2, 2, 2, 2, 2, // row 2
            3, 3, 3, 3, 3, 3, 3, 3, // row 3
        ];
        flip_rows(&mut rgba, 2, 4);
        // After flip: rows should be 3,2,1,0.
        assert_eq!(rgba[0], 3);
        assert_eq!(rgba[8], 2);
        assert_eq!(rgba[16], 1);
        assert_eq!(rgba[24], 0);
    }

    #[test]
    fn flip_rows_single_row_is_noop() {
        let mut rgba = vec![9, 9, 9, 9];
        flip_rows(&mut rgba, 1, 1);
        assert_eq!(rgba, vec![9, 9, 9, 9]);
    }

    #[test]
    fn mostly_white_icon_detects_generic_document_like_icons() {
        let white = DecodedIcon {
            rgba: [245, 245, 245, 255].repeat(32 * 32),
            w: 32,
            h: 32,
        };
        assert!(is_mostly_white_icon(&white));

        let blue = DecodedIcon {
            rgba: [0, 120, 255, 255].repeat(32 * 32),
            w: 32,
            h: 32,
        };
        assert!(!is_mostly_white_icon(&blue));
    }

    #[test]
    #[ignore = "diagnostic: writes strategy PNGs for local Start Menu shortcuts"]
    fn probe_shortcut_icon_strategies() {
        use std::fs;
        use std::io::Write;

        let _com = ComScope::new();
        let out_dir = PathBuf::from("target").join("icon-probe");
        fs::create_dir_all(&out_dir).unwrap();

        let mut csv = String::from(
            "name,path,strategy,width,height,opaque,whiteish,avg_alpha,bounds_x,bounds_y,bounds_w,bounds_h\n",
        );

        for shortcut in enumerate_start_menu() {
            let safe_name = sanitize_filename(&shortcut.name);
            let link = load_shell_link(&shortcut.path);

            if let Some(icon) = get_path_hicon(&shortcut.path).and_then(icon_to_decoded) {
                record_probe(&out_dir, &mut csv, &safe_name, &shortcut, "lnk", &icon);
            }

            if let Some(link) = link.as_ref() {
                if let Some(target) = resolve_link_target(link) {
                    if let Some(icon) = get_path_hicon(&target).and_then(icon_to_decoded) {
                        record_probe(&out_dir, &mut csv, &safe_name, &shortcut, "target", &icon);
                    }
                }

                if let Some(icon) = get_link_pidl_image(link) {
                    record_probe(
                        &out_dir,
                        &mut csv,
                        &safe_name,
                        &shortcut,
                        "pidl_image",
                        &icon,
                    );
                }
            }

            if let Some(target) = find_exe_by_shortcut_name(&shortcut.path) {
                if let Some(icon) = get_path_hicon(&target).and_then(icon_to_decoded) {
                    record_probe(&out_dir, &mut csv, &safe_name, &shortcut, "name_exe", &icon);
                }
            }
        }

        let csv_path = out_dir.join("summary.csv");
        let mut file = fs::File::create(&csv_path).unwrap();
        file.write_all(csv.as_bytes()).unwrap();
        eprintln!("wrote {}", csv_path.display());
    }

    #[test]
    #[ignore = "diagnostic: verifies local document shortcuts keep concrete icons"]
    fn probe_document_shortcuts_keep_icons() {
        let _com = ComScope::new();
        let shortcuts = enumerate_start_menu();

        for name in [
            "Documentation for Desktop Apps",
            "Documentation for UWP Apps",
            "Sample Desktop Apps",
            "Sample UWP Apps",
            "Tools for Desktop Apps",
            "Tools for UWP Apps",
            "Documentation",
            "Release Notes",
            "VideoLAN Website",
        ] {
            if let Some(shortcut) = shortcuts.iter().find(|shortcut| shortcut.name == name) {
                assert!(
                    extract_icon_from_lnk(&shortcut.path).is_some(),
                    "{name} should remain listed with a concrete document/browser icon"
                );
            }
        }
    }

    fn icon_to_decoded(hicon: HICON) -> Option<DecodedIcon> {
        let _guard = IconGuard(hicon);
        hicon_to_rgba(hicon)
    }

    fn record_probe(
        out_dir: &Path,
        csv: &mut String,
        safe_name: &str,
        shortcut: &Shortcut,
        strategy: &str,
        icon: &DecodedIcon,
    ) {
        let png = out_dir.join(format!("{safe_name}__{strategy}.png"));
        image::save_buffer(&png, &icon.rgba, icon.w, icon.h, image::ColorType::Rgba8).unwrap();

        let stats = icon_stats(icon);
        csv.push_str(&format!(
            "{:?},{:?},{},{},{},{},{},{:.1},{},{},{},{}\n",
            shortcut.name,
            shortcut.path.display().to_string(),
            strategy,
            icon.w,
            icon.h,
            stats.opaque,
            stats.whiteish,
            stats.avg_alpha,
            stats.bounds.0,
            stats.bounds.1,
            stats.bounds.2,
            stats.bounds.3,
        ));
    }

    struct IconStats {
        opaque: usize,
        whiteish: usize,
        avg_alpha: f32,
        bounds: (u32, u32, u32, u32),
    }

    fn icon_stats(icon: &DecodedIcon) -> IconStats {
        let mut opaque = 0usize;
        let mut whiteish = 0usize;
        let mut alpha_sum = 0usize;
        let mut min_x = icon.w;
        let mut min_y = icon.h;
        let mut max_x = 0u32;
        let mut max_y = 0u32;

        for y in 0..icon.h {
            for x in 0..icon.w {
                let idx = ((y * icon.w + x) * 4) as usize;
                let r = icon.rgba[idx];
                let g = icon.rgba[idx + 1];
                let b = icon.rgba[idx + 2];
                let a = icon.rgba[idx + 3];
                alpha_sum += a as usize;
                if a > 10 {
                    opaque += 1;
                    if r > 220 && g > 220 && b > 220 {
                        whiteish += 1;
                    }
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }

        let bounds = if opaque == 0 {
            (0, 0, 0, 0)
        } else {
            (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
        };

        IconStats {
            opaque,
            whiteish,
            avg_alpha: alpha_sum as f32 / (icon.w * icon.h) as f32,
            bounds,
        }
    }

    fn sanitize_filename(s: &str) -> String {
        let mut out = String::new();
        for ch in s.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                out.push(ch);
            } else {
                out.push('_');
            }
        }
        if out.is_empty() {
            "shortcut".to_string()
        } else {
            out
        }
    }
}
