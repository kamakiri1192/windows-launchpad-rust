# launchpad-windows docs

- [../ARCHITECTURE.md](../ARCHITECTURE.md) - target New Architecture,
  boundaries, data flow, and extension rules.
- [DF_REARCHITECTURE_PLAN.md](DF_REARCHITECTURE_PLAN.md) - proposed Dynamic
  Feature-ready refactor plan and migration phases.
- [GLASS_FOCUS_VEIL.md](GLASS_FOCUS_VEIL.md) - フォルダ表示時に下層シーンを
  ぼかすGlass Focus Veilの目的、描画順、GPU構成、調整方法、視覚QA項目。
- [LIQUID_GLASS_PER_SURFACE_BLUR.md](LIQUID_GLASS_PER_SURFACE_BLUR.md) -
  `GlassSurface` ごとの背景ブラー、完成済みlane別blur出力、透明ウィンドウでの
  backdrop replacement、context menu のGPUコストと視覚QA。
- [INPUT_PASSTHROUGH_REQUIREMENTS.md](INPUT_PASSTHROUGH_REQUIREMENTS.md) —
  ページフレーム外のクリック、ドラッグ、縦スクロール、ホバーの入力要件。
- [INPUT_PASSTHROUGH_TECHNICAL_RESEARCH.md](INPUT_PASSTHROUGH_TECHNICAL_RESEARCH.md) —
  Windows / macOS の実現方式調査、推奨アーキテクチャ、native probe を使う自動検証計画。

- [KEYBINDINGS.md](KEYBINDINGS.md) — runtime debug / tuning keys, the
  `blur_radius` → pyramid depth mapping, and Windows transparency notes.
- [BOTTOM_CONTROL.md](BOTTOM_CONTROL.md) — the morphing bottom-center control
  (search pill ↔ page indicator ↔ search field): state machine, visuals, and
  interaction summary.
- [STARTUP_PERFORMANCE.md](STARTUP_PERFORMANCE.md) — launch pipeline redesign,
  UI/worker responsibilities, and startup timing logs.
- [ICON_CACHE.md](ICON_CACHE.md) — SQLite icon cache: location, schema,
  invalidation rules, and corruption recovery.
- [ICON_COMPATIBILITY_CI.md](ICON_COMPATIBILITY_CI.md) — macOS 14 / 15 / 26
  icon capture, pixel and outer-shape comparison, artifacts, and label trigger.
- [APP_REFRESH.md](APP_REFRESH.md) — live Start Menu change detection
  (added / updated / removed) and click-stability via stable app ids.
- [EDIT_MODE_VISUAL_QA.md](EDIT_MODE_VISUAL_QA.md) — manual visual QA for
  edit-mode reordering, screenshots, persistence checks, and hit-test logic.
