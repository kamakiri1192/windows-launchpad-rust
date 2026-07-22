# Edit Mode Visual QA

> 長押し・ドラッグ・フォルダページスワイプを非表示で自動再生し、連番 PNG
> と manifest を取得する場合は [GPU_SEQUENCE_QA.md](GPU_SEQUENCE_QA.md) を参照してください。
> この文書の単発キャプチャと手動 QA も引き続き利用できます。

This document records the manual visual QA process used for the iOS-style
edit-mode work: long-press entry, wiggle/lift visuals, drag reordering, edge
autoscroll, Done exit, delete-badge hiding, and persistence across launches.

The app is a transparent, capture-excluded Windows overlay, so ordinary
screenshot tools can produce misleading results unless the app is launched with
the right environment.

## What To Validate

For edit-mode changes, cover these behaviors together rather than as isolated
click tests:

- Long-pressing an app enters edit mode.
- Icons wiggle, the dragged icon lifts/scales, and delete badges appear.
- Dragging an icon to another visible cell reorders the grid immediately.
- Dragging to an empty cell on the current page works.
- Dragging to the rightmost two columns works.
- Holding a dragged icon near the page-frame edge autoscrolls by one page.
- Clicking Done exits edit mode and removes badges/wiggle.
- Clicking a delete badge hides that app from the visible grid.
- Reorder and hidden-app state survive a full process restart.

## Required Environment

Use a release build:

```powershell
cargo build --release
```

Set these environment variables for QA launches:

```powershell
$env:LAUNCHPAD_ALLOW_SCREENSHOT = '1'
$env:LAUNCHPAD_DEBUG = '1'
```

`LAUNCHPAD_ALLOW_SCREENSHOT=1` is required because normal windows are excluded
from screen capture. Without it, screenshot results can show the desktop or the
capturing app instead of Launchpad.

`LAUNCHPAD_DEBUG=1` writes `%LOCALAPPDATA%\Launchpad\debug.log`. Release builds
use the Windows subsystem, so stdout/stderr are not useful during manual QA.

For repeatable testing that does not modify the user's real Launchpad cache,
also point `LOCALAPPDATA` at a temporary directory:

```powershell
$tmp = Join-Path (Resolve-Path .\target).Path 'codex-localappdata'
Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$env:LOCALAPPDATA = $tmp
```

## Screenshot Setup

Move the window to a known screen rectangle before capturing. This avoids
capturing through another transparent window and gives stable coordinates for
pointer automation.

Example:

```powershell
$exe = Join-Path (Resolve-Path .).Path 'target\release\launchpad-windows.exe'
$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Milliseconds 2500

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win32Qa {
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int X, int Y, int nWidth, int nHeight, bool bRepaint);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
'@

$p.Refresh()
[Win32Qa]::MoveWindow($p.MainWindowHandle, 0, 0, 1280, 800, $true) | Out-Null
[Win32Qa]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
```

Capture the screen with `System.Drawing.CopyFromScreen`:

```powershell
Add-Type -AssemblyName System.Drawing
$bmp = New-Object Drawing.Bitmap 1280,800
$g = [Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0,0,0,0,$bmp.Size)
$bmp.Save((Join-Path (Resolve-Path .\target).Path 'qa.png'), [Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
```

On high-DPI or 4K monitors, do not assume `1280x800` is the whole window. That
only captures the upper-left part of a 4K desktop. Query the moved window rect
and capture that physical-pixel size:

```powershell
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win32RectQa {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
}
'@

$rect = New-Object Win32RectQa+RECT
[Win32RectQa]::GetWindowRect($p.MainWindowHandle, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
$bmp = New-Object Drawing.Bitmap $w,$h
$g = [Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($rect.Left,$rect.Top,0,0,$bmp.Size)
$bmp.Save((Join-Path (Resolve-Path .\target).Path 'qa.png'), [Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
```

## GPU Self-Capture (no foreground access needed)

`CopyFromScreen` requires the launcher window to be in the foreground, which
CI / sandbox environments often block. When that fails (`SetForegroundWindow`
returns false, or `CopyFromScreen` raises "invalid handle"), use the built-in
GPU self-capture path instead: it copies the rendered surface texture directly
to a host buffer and writes a PNG, with no foreground dependency.

Set `LAUNCHPAD_QA_SHOT_FILE` to a trigger file path, then write the desired
output PNG path into that file. The app polls the trigger once per frame and,
when it finds a non-empty path, saves the next rendered frame there and clears
the trigger (so each write captures exactly one frame).

```powershell
$trigger = Join-Path (Resolve-Path .\target).Path 'qa-trigger.txt'
$env:LAUNCHPAD_QA_SHOT_FILE = $trigger
# ... launch the app with LAUNCHPAD_ALLOW_SCREENSHOT=1 ...

# Capture the current frame to target/qa-frame.png:
Set-Content -Path $trigger -Value (Join-Path (Resolve-Path .\target).Path 'qa-frame.png')
Start-Sleep -Milliseconds 500  # wait one frame
```

This is how the Phase 2 bottom-control slice was screen-verified inside a
sandbox that refused foreground access: a driver script performs pointer /
keyboard automation (or drives the app's own state for deterministic checks),
writes the trigger before each milestone, and reads back the PNG.

The capture is the physical-pixel surface size (e.g. 1920×1200 at 150% DPI),
so divide by the scale factor when comparing to logical coordinates.

## Coordinate Notes

The app uses physical pixels internally. Windows pointer APIs and screenshots
may be in logical screen coordinates depending on monitor DPI. In the observed
150% DPI environment, screen coordinates needed to be multiplied by 1.5 before
they matched `winit` pointer coordinates in debug logs.

Use the debug log to confirm this before trusting automated pointer movement.
For example, a click near logical screen `(322, 98)` can appear as roughly
physical `(483, 147)` in app logs.

When a drag appears visually correct but reorder does not happen, check whether
the logged pointer is landing in the label area or outside the tile cell. Edit
drop hit testing intentionally excludes labels.

## Reorder Persistence Check

Recommended flow:

1. Launch with temporary `LOCALAPPDATA`, `LAUNCHPAD_ALLOW_SCREENSHOT=1`, and
   `LAUNCHPAD_DEBUG=1`.
2. Capture the initial screen.
3. Long-press the first tile until edit mode appears.
4. Drag it to another tile cell, then release.
5. Capture the reordered screen.
6. Click Done.
7. Stop the process.
8. Start the app again with the same temporary `LOCALAPPDATA`.
9. Capture the restarted screen and verify the order is unchanged.

Also inspect the cache directly. `app_order` is stored in the SQLite `kv` table
as a compact binary list: `count:u32`, followed by repeated
`len:u32 + UTF-8 app_id` entries.

Example parser:

```powershell
$py = 'C:\Users\kamak\.cache\codex-runtimes\codex-primary-runtime\dependencies\python\python.exe'
@'
import os, sqlite3, struct
path = os.path.join(os.environ['LOCALAPPDATA'], 'Launchpad', 'cache.sqlite3')
con = sqlite3.connect(path)
blob = con.execute("select value from kv where key='app_order'").fetchone()[0]
pos = 0
count = struct.unpack_from('<I', blob, pos)[0]
pos += 4
for i in range(min(count, 12)):
    n = struct.unpack_from('<I', blob, pos)[0]
    pos += 4
    app_id = blob[pos:pos+n].decode('utf-8')
    pos += n
    print(i, app_id)
'@ | & $py -
```

The important logic check is that persisted IDs must remain pending when loaded
before the Start Menu scan has inserted records. If `set_order()` drops unknown
IDs during startup, the UI can look correct during the same session but revert
after restart.

## Delete Badge Persistence Check

Recommended flow:

1. Launch with a fresh temporary `LOCALAPPDATA`.
2. Long-press an app to enter edit mode.
3. Click the app's top-left delete badge.
4. Verify the app disappears and later apps close the gap.
5. Stop and restart the process with the same temporary `LOCALAPPDATA`.
6. Verify the app is still hidden.
7. Parse `hidden_ids` from the same `kv` table using the binary list format
   described above.

The delete badge hit-test and shader badge position must agree. The current
badge is top-left in both Rust hit testing and WGSL rendering. If either side
uses top-right or a different radius, screenshots can show a badge that clicks
somewhere else.

## Rightmost Column Regression

Each page's grid is centered in the viewport, while page spacing uses the
narrower liquid-glass panel width. The rendered x coordinate is:

```text
x = page * page_width + margin_left + col * (tile_size + gap)
```

The hit-test must mirror that formula. A tempting shortcut is:

```text
page = floor(content_x / page_width)
x_in_page = content_x - page * page_width - margin_left
```

That is wrong when `margin_left + grid_width` extends beyond `page_width`. In
that case, the rightmost one or two columns of page N are misclassified as page
N+1 and cannot be used as drop targets.

The regression test should verify both rightmost columns:

```rust
for col in [g.cols - 2, g.cols - 1] {
    let x = g.margin_left + col as f32 * step_x + g.tile_size * 0.5;
    assert_eq!(
        g.hit_test_tile_cell(vw, x, y, 0.0, g.total_tiles()),
        Some(col),
        "column {col} should be reachable"
    );
}
```

## Edge Autoscroll Logic

Edge autoscroll should start when the dragged icon is held in the page-frame
gutter, not while it is over the rightmost tile columns. The page frame is wider
than the grid, so the autoscroll zone should be clamped to the space between
the frame edge and the grid edge.

The logic should use:

```text
left_zone = min(configured_zone, grid_left - frame_left)
right_zone = min(configured_zone, frame_right - grid_right)
```

This preserves page-edge dragging while keeping normal drop targets on the
right side of the grid reachable.

## Final Checklist

Before considering an edit-mode visual change complete:

- Run `cargo fmt`.
- Run `cargo test`.
- Run `cargo clippy --all-targets --all-features`.
- Run `cargo build --release`.
- Launch the release exe with screenshot capture allowed.
- Capture before/after/restart screenshots for reorder persistence.
- Capture before/after/restart screenshots for delete hiding.
- Capture an edge-hold screenshot showing page autoscroll.
- Confirm no `launchpad-windows.exe` process is left running from temporary
  QA unless intentionally relaunched for the user.

## Phase 8 Folder QA

Folder QA uses the same release build, temporary `LOCALAPPDATA`, screenshot
opt-in, and GPU self-capture trigger described above. Seed the temporary
cache's `kv.launcher_state` JSON with at least these cases so the result is
repeatable without changing the user's real layout:

- an 11-child folder for pagination and a two-item final page;
- a 3-child folder for the centered incomplete row and narrow dynamic panel;
- a long Japanese folder name for title fitting;
- normal app items before, between, and after folder items.

Validate the following on screen:

1. The closed folder preview contains ordered 3x3 mini icons and is interleaved
   with normal apps.
2. Clicking it opens a Liquid Glass modal; the background dims/scales and the
   source tile remains spatially connected to the opening panel.
3. Capture opening and closing at roughly 100 ms intervals with
   `LAUNCHPAD_QA_SHOT_FILE`. Confirm the panel rectangle/radius and child icons
   converge exactly at both endpoints.
4. Press `Esc` during opening and capture the next frames. The spring must
   reverse from its current position, with no snap to an endpoint.
5. Switch an 11-child folder to its second page. Confirm the final two children
   are centered and the page indicator changes; return to page one.
6. Open the 3-child folder and confirm its panel uses a centered single row.
7. Rename the folder, press Enter, close/reopen, and inspect
   `kv.launcher_state` to confirm persistence. Start another rename and press
   `Esc`; the committed name must remain unchanged. Exercise Japanese text and
   IME manually if the automation driver cannot emit IME commit events.
8. Click outside the panel. It must close without launching or activating the
   item underneath.
9. Open search and enter a query. Results must be flat apps; folders and folder
   panels must not appear.
10. Long-press and drag to cover app-on-app creation, app-into-folder,
    child reorder, child-to-folder, and child-to-top-level. Verify one-child
    folders dissolve in their original slot.

Some Windows automation APIs expose only an atomic drag and cannot express the
launcher's long-press-then-move gesture. When that limitation applies, record
the five drag scenarios as not screen-verified and rely on the deterministic
folder feature/domain/layout tests; do not report them as manually verified.

Before finishing, close the temporary launcher process and run:

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features
cargo build --release
```
