# Startup performance

This document describes the launch-time architecture of the launcher: the old
(blocking) pipeline, the new (incremental) pipeline, and the timing logs that
make "where does launch time go?" answerable from a run.

## Old pipeline (before this work)

Everything that produced the first visible frame ran **synchronously on the UI
thread**:

```text
1. window creation
2. renderer initialization (wgpu device/surface/pipelines)
3. full Start Menu .lnk enumeration
4. full icon extraction (Shell / GDI / COM, per shortcut)
5. full icon normalization (resize → 128×128 RGBA)
6. atlas packing (one big texture)
7. GPU texture upload
8. first frame rendered
```

Steps 3–7 are the expensive ones, and they all happened *before* the window was
useful. On a machine with many shortcuts — or slow Shell/GDI — the window sat
blank (or didn't appear at all) for hundreds of milliseconds to seconds.

## New pipeline

The window paints as soon as the renderer is ready. App-list discovery, icon
extraction, and caching all happen in the background and are reflected
incrementally:

```text
1. window creation
2. renderer initialization
3. FIRST FRAME (empty / loading state — placeholders + labels later)
4. app list discovered (refresh watcher's Initial snapshot) → tiles + labels
5. cached icons applied immediately (no Shell/GDI)
6. missing / stale icons extracted in the background (icon worker)
7. each icon reflected to the UI as it arrives (one cell at a time)
8. Start Menu changes while running are diffed and reflected live
```

### Thread responsibilities

| Thread            | Owns / does                                                      | Must NOT do                         |
| ----------------- | ---------------------------------------------------------------- | ----------------------------------- |
| **UI thread**     | window, renderer, app registry mutation, atlas blits, redraw     | Shell/GDI/COM, heavy I/O, big atlas rebuilds |
| **icon worker**   | `.lnk` → RGBA extraction, normalization, cache writes            | touch UI state, hold Win32 handles across the channel |
| **refresh watcher** | periodic Start Menu rescans, diff computation                  | extract icons, block the UI         |
| **inbox forwarder** | drains worker+watcher channel, pushes into shared inbox, wakes UI | —                                  |

The icon worker initializes COM per-thread (`COINIT_APARTMENTTHREADED`) and
processes requests one at a time. Only ownable Rust data crosses back to the UI
(`AppId`, normalized `DecodedIcon`, error string) — never `HICON`, `HBITMAP`,
or `HDC`. A panic inside extraction is caught with `catch_unwind` and reported
as a per-app failure, so one bad shortcut can't kill the worker or freeze the
UI.

### First paint without icons

`App::resumed` builds the renderer, performs one `relayout()` with an empty
registry (so the GPU has valid instance buffers), and requests a redraw **before**
any icon work. The refresh watcher's first scan arrives moments later via
`RefreshMessage::Initial`, which populates the registry and triggers a second
`relayout()` — now with real labels and placeholder color tiles. Icons then
trickle in.

### Incremental icon application

Each icon (cached or freshly extracted) flows through `App::apply_icon`, which:

1. writes the normalized RGBA into the app's **fixed slot** in the CPU atlas
   (`IconAtlas::write_icon`);
2. pushes just that cell to the GPU with `Renderer::write_icon_cell`
   (a single `queue.write_texture` — no full atlas rebuild);
3. records the UV in the registry and marks the state `Cached` / `Loaded`;
4. rebuilds the (small) icon instance buffer so the new UV is sampled;
5. requests a redraw.

This keeps the UI thread work per icon to a few hundred floats + one texture
copy, so scrolling/clicking/exit never stall while icons are loading.

## Timing logs

`StartupTimer` records `Instant`-based phase marks and prints them to stderr.
Every line is prefixed for easy grepping. Each line shows the step delta and
the total elapsed since process start, e.g.:

```text
startup: process start in 0ms (total 0ms)
startup: window creation in 40ms (total 40ms)
startup: renderer initialization in 180ms (total 220ms)
startup: first redraw requested in 3ms (total 223ms)
startup: first frame rendered in 5ms (total 228ms)
startup: app list enumeration (84 apps) in 60ms (total 290ms)
icon-cache: cache open in 2ms (total 292ms)
icon-cache: cache load in 1ms (total 293ms)
icon-cache: cached icon apply (80 icons) in 8ms (total 301ms)
icon-worker: queue extraction (4 icons) in 0ms (total 301ms)
icon-worker: extracted icon app_id=... (18ms) (total 320ms)
startup: atlas + GPU texture upload in 6ms (total 326ms)
app-refresh: initial scan (84 apps) in 55ms (total 55ms)
app-refresh: detected diff added=1 updated=2 removed=0 in 50ms (total 12050ms)
app-refresh: app list refresh (added=1 updated=2 removed=0) in 2ms (total 12052ms)
```

### Prefixes

| Prefix         | Meaning                                            |
| -------------- | -------------------------------------------------- |
| `startup:`     | one-time launch phases (window, renderer, frames)  |
| `icon-cache:`  | SQLite cache open / load / apply                   |
| `icon-worker:` | per-icon extraction + queueing                     |
| `app-refresh:` | Start Menu scans + diffs + registry refresh        |

### Phases logged

- `process start`
- `window creation`
- `renderer initialization`
- `first redraw requested`
- `first frame rendered`
- `app list enumeration` (Start Menu shortcut scan)
- `cache open`
- `cache load`
- `cached icon apply`
- `atlas + GPU texture upload`
- `extracted icon` (per item, with `app_id=` and ms)
- `queue extraction`
- `initial scan` / `detected diff` / `app list refresh` (refresh watcher)

Logs always go to stderr (independent of `RUST_LOG`), so they're visible in a
bug report without configuration.

## Known limitations

- The icon worker processes one shortcut at a time; on a cold cache with many
  shortcuts, the tail icons arrive over several seconds. Parallelism is a
  future option (each worker needs its own COM apartment).
- The refresh watcher polls (~10s); event-driven `ReadDirectoryChangesW` would
  be lower-latency (see [APP_REFRESH.md](APP_REFRESH.md)).
- Atlas slots are not compacted after removals, so a long-running launcher that
  sees lots of add/remove churn will grow the atlas. Growth is bounded by the
  GPU's max 2D texture dimension.

## Future improvements

- Parallel icon extraction (worker pool, one COM apartment each).
- `ReadDirectoryChangesW`-based refresh watcher with poll fallback.
- Re-validate cached icons in the background even when "valid" (defense in
  depth against a stale cache).
- Atlas compaction after sustained removal churn.
