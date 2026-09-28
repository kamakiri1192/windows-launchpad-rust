# launchpad-windows docs

- [DATABASE_MIGRATION_PLAN.md](DATABASE_MIGRATION_PLAN.md) — 承認済みのDB統一・`rusqlite_migration`導入方針。
- [INITIAL_FOLDER_LAYOUT_SPEC.md](INITIAL_FOLDER_LAYOUT_SPEC.md) — 初期フォルダ配置の非AI仕様。
  Windows / macOSの19種類のプリセット候補、初回判定、保存、取り消し、受け入れ条件。
- [AI_FOLDER_ASSIST_SPEC.md](AI_FOLDER_ASSIST_SPEC.md) — 任意のAI整理補助仕様。
  ルールとの境界、外部送信、初回許可と撤回、プレビュー、Store要件、検証条件。

- [../ARCHITECTURE.md](../ARCHITECTURE.md) - target New Architecture,
  boundaries, data flow, and extension rules.
- [DF_REARCHITECTURE_PLAN.md](DF_REARCHITECTURE_PLAN.md) - proposed Dynamic
  Feature-ready refactor plan and migration phases.
- [GLASS_FOCUS_VEIL.md](GLASS_FOCUS_VEIL.md) - フォルダ表示時に下層シーンを
  ぼかすGlass Focus Veilの目的、描画順、GPU構成、調整方法、視覚QA項目。
- [LIQUID_GLASS_PER_SURFACE_BLUR.md](LIQUID_GLASS_PER_SURFACE_BLUR.md) -
  上層パーツ直前の完成シーンを背景にするLiquid Glassブラーの設計。checkpoint、
  desktopへのflatten、lane別completed blur、backdrop replacement、別パーツへの
  導入手順、GPU resource lifecycle、失敗例、視覚QAをまとめる。
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
- [CONTEXT_MENU_ICON_ASSETS.md](CONTEXT_MENU_ICON_ASSETS.md) — SVGを透過PNGへ
  変換し、コンテキストメニューのテクスチャアイコンとして取り込む手順。
- [APP_REFRESH.md](APP_REFRESH.md) — live Start Menu change detection
  (added / updated / removed) and click-stability via stable app ids.
- [EDIT_MODE_VISUAL_QA.md](EDIT_MODE_VISUAL_QA.md) — manual visual QA for
  edit-mode reordering, screenshots, persistence checks, and hit-test logic.
