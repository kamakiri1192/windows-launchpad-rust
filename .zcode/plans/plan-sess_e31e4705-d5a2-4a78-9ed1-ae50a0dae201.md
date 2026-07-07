## Phase 4: Edit Mode Vertical Slice — 実装計画

### 方針(ユーザー承認済み)
- **1 PR・複数コミット**。コミット順: inventory追記 → layout/edit_mode → features/edit_mode → main.rs adapter → tests → docs/log。
- **PendingPress は main.rs に残留**。launch/passthrough/scroll-drag も担るため Phase 5 の app shell 抽出まで完全移行できない。edit-mode 側の判定(long-press / edit-press / slop / outside_glass)だけ pure 関数に切り出し、main.rs の PendingPress 構築と launch/passthrough 分岐は現状維持。残した adapter は docs/DF_REARCHITECTURE_LOG.md に明記。
- **behavior-preserving 最優先**。計算結果を変えず、モジュール境界と依存方向だけ整える。`scroll.rs` 物理 / 検索 / ボトムコントロール状態機械 / settings / click passthrough は触らない。

---

### コミット 1: docs/DF_CURRENT_BEHAVIOR_INVENTORY.md に Edit Mode セクション追記
ソース: `src/main.rs` の edit-mode 系メソッド + `layout/grid.rs` の badge/drop ジオメトリ。
以下を含める:
- long press entry(`LONG_PRESS_THRESHOLD` / `CLICK_SLOP_PHYS` / `outside_glass` では入らない)
- pending press と scroll-drag / launch-click / passthrough の優先関係
- edit entry 時の scroll cancel / wiggle reset / long-pressed app lift
- icon wiggle / dragged icon lift+scale+1.15 / pointer-follow / draw-on-top / FLAG_DRAG の frame-clip bypass
- edit badge hide の visual / hit precedence(badge が drag より優先)
- edit-mode press: badge hide / app drag / empty-click exit
- edit-mode release: drop + commit + persist(SortOrder::Manual + user_order + hidden)
- CursorLeft 時の drag finalize + pending press cancel
- Esc / Done / settings gear / focus loss(編集中は auto-hide 無効)
- live reorder / empty-cell drop / rightmost columns / label area は drop 対象外
- edge autoscroll zone(72px clamp to gutter, 24px floor, panel_w*0.25 cap)
- hidden apps: order preservation / 末尾に格納 / SortOrder::Manual
- persistence of order / hidden across restart
- tile springs / slide animation の現行役割
- Phase 4 で残す adapter(TileAnim / TileInstance / IconInstance / renderer badge source / lift_dragged_instances / tile_springs)

### コミット 2: layout/edit_mode.rs 新設(純粋幾何 + hit regions)
`layout/grid.rs` を**最小拡張**しつつ、edit-mode 固有の幾何は新しい `layout/edit_mode.rs` に集約。ライブラリターゲットでコンパイル(`wgpu`/`winit`/`ScrollBounds` に依存しない)。Phase 2/3 のパターン踏襲。

`layout/grid.rs` に追加:
- `edit_badge_center(viewport_w, scroll_x, idx)` — badge 中心座標(現在 `main.rs::badge_hit` 内の inset 計算を純粋化)
- `edit_badge_hit_test(viewport_w, x, y, scroll_x, idx, radius, slop)` — badge 円ヒットテスト

`layout/edit_mode.rs` 新設:
- `EditModeGeometry` / `EditModeLayoutInput`:
  - `viewport`, `scroll_x`, `visible_count`, `total_tiles`, `editing`, `drag_app: Option<usize>`(visible index), `scale`, `layout: &GridLayout` 参照
- 純粋関数:
  - `badge_geometry(idx) -> (center, radius, hit_radius+slop)` — render と hit を同じ計算から出す
  - `badge_hit(idx, x, y) -> bool`
  - `drop_cell_at(x, y) -> Option<usize>` — `hit_test_tile_cell` の薄いラッパ(label 排除済み)
  - `label_area_is_not_drop_target` のドキュメント化
  - `edge_autoscroll_zone(panel, grid_left, grid_right, scale) -> (left_zone, right_zone)` — gutter clamp 含む
  - `edge_autoscroll_target(drag_x, drag_y, current_page, page_count, zones, panel) -> Option<usize>`
  - `reorder_insert_index(visible_len, drag_pos, target_idx) -> Option<usize>` — 純粋計算(`live_reorder` の判定部分)
  - `badge_hit_precedence` のドキュメント(badge > drag)

重複ジオメトリは作らない: edit settings gear / Done は **Phase 2 の `layout::bottom_control` 境界を再利用**。`main.rs` の `handle_control_click` の edit 分岐は `BottomControlPointerIntent` をそのまま使う。

### コミット 3: features/edit_mode/ 新設(状態遷移・intent・outcome)
新ディレクトリ `src/features/`。`features/edit_mode/` のみ(Phase 5 まで他 feature は作らない)。

`features/edit_mode/mod.rs`:
- 状態型 `EditModeState { editing: bool, drag_app: Option<AppId>, drag_x, drag_y, wiggle_phase: f32 }`
- intent(純粋判定関数、引数はスナップショット):
  - `should_enter_from_long_press(press: &PressSnapshot, now, slop, threshold, pointer) -> bool`
  - `edit_press_classify(hit: GridHit, badge_hit: bool) -> EditPressIntent { HideApp, DragApp, EmptyExit, Noop }`
  - `edit_release_outcome(has_drag) -> EditReleaseIntent { CommitDrop, Noop }`
- outcome(副作用の要求、実行はしない):
  - `EditModeCommand { RequestRedraw, Relayout, PersistUserOrder, PersistHidden, PersistSettings, SettleToPage(usize), SetEditing(bool), SetDragApp(Option<AppId>), HideApp(AppId) }`
  - `enter(state, app_index, visible_ids, pointer) -> Vec<EditModeCommand>`(scroll cancel / wiggle reset / lift を含む)
  - `exit(state) -> Vec<EditModeCommand>`(commit + persist 含む)
  - `apply_reorder(state, order_snapshot, drag_id, insert_idx) -> (Vec<AppId>, Vec<EditModeCommand>)`(純粋な order 計算)
- Phase 5 の全体 `AppAction` / `AppCommand` は作らない。edit-mode 専用の狭い型。

`features/edit_mode/state.rs`: 状態構造体とミューテータ。
`features/edit_mode/tests.rs`: 純粋判定の unit test(コミット 5 で拡張)。

依存方向を守る: `features/edit_mode` → `layout/edit_mode` + `layout/grid`(純粋型) のみ。`AppId` は domain だが現状 `src/app_id.rs`(binary)にあるため、**Phase 4 では `features/edit_mode` は binary モジュールとして配置**(`mod features;` を `main.rs` に追加)。`app_id` に依存してもバイナリ内なので問題なし。`domain/` への移行は Phase 7/8。

### コミット 4: main.rs を adapter 化
`App` のフィールド(`editing`, `drag_app`, `drag_x`, `drag_y`, `wiggle_phase`)は残す(レンダー/スクローラが直接読むため)。edit-mode 判断ロジックを `features/edit_mode` に寄せる:

- `begin_grid_press` / `maybe_long_press_into_edit`: long-press 判定を `features::edit_mode::should_enter_from_long_press` 経由に。
- `enter_edit_mode` / `exit_edit_mode`: 内部で `features::edit_mode::enter/exit` を呼び、返ってきた `EditModeCommand` を main.rs で実行(scroll cancel / relayout / persist / redraw)。ロジックは feature 側、実行は app 境界。
- `live_reorder` / `reorder_by_index`: order 計算を `features::edit_mode::apply_reorder` に、`registry.set_order` は main.rs。
- `badge_hit`: `layout::edit_mode::badge_hit` へ。
- `edit_drop_index_at_pointer`: `layout::edit_mode::drop_cell_at` へ。
- `maybe_autoscroll_edit_drag`: zone/target 計算を `layout::edit_mode::edge_autoscroll_*` へ。`scroller.settle_to_page` 呼び出しは main.rs。
- `commit_reorder`: persist(SortOrder::Manual + user_order) は main.rs、reorder 計算は feature 側。
- MouseInput press/release の edit 分岐、CursorLeft、CursorMoved の edit 分岐: 判定は feature/layout、副作用(registry mutation / renderer upload / scroller)は main.rs。

**GPU-facing adapter として残す**: `TileAnim`, `TileInstance`, `IconInstance`, `edit_anim`, `lift_dragged_instances`, `tile_springs`, renderer badge source, `step_edit_control_width`, `edit_visual_progress`。これらは docs/DF_REARCHITECTURE_LOG.md に明記。

### コミット 5: tests 追加
`layout/edit_mode.rs` のテスト(ライブラリ):
- badge geometry が render と hit で一致
- badge hit target と badge precedence(badge > drag)
- drop cell hit(rightmost 2 columns / empty-cell / label area は非対象)
- edge autoscroll zone と gutter clamp / 24px floor / panel_w*0.25 cap
- edge autoscroll target(current_page=0/最終ページ の境界)
- reorder insert index 計算

`features/edit_mode/tests.rs`(binary):
- long-press entry / no-entry(outside_glass / slop 超え / しきい値未満)
- drag-vs-click slop 分類
- edit entry with app lift と empty long-press(no lift)
- badge hit precedence over drag
- hide app behavior(order 末尾移動)と hidden persistence command 発行
- empty-cell drop / rightmost 2 columns / label area 非対象
- hidden apps preserved after visible order
- commit reorder が SortOrder::Manual 設定 + persistence command 発行
- Done(EditGear) routing(intent 経由)
- tile animation flags(FLAG_WIGGLE / FLAG_DRAG / scale=1.15)

### コミット 6: docs/DF_REARCHITECTURE_LOG.md 更新
実施内容 / 変更ファイル / behavior-preserving の根拠(計算を変えず境界だけ移動) / 残した adapter / 実行 validation / Screen Verification Gate 結果 / 確認できなかった項目と理由。

---

### 必須 validation
1. `cargo fmt`
2. `cargo test`(lib + bin + WGSL)
3. `cargo clippy --all-targets --all-features`
4. `cargo build --release`
5. `codex review --base main`(edit pointer routing / reorder / persistence 触った後)

### Screen Verification Gate
`LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`, temporary `LOCALAPPDATA`。foreground 不可なら `LAUNCHPAD_QA_SHOT_FILE` の GPU self-capture。
- first frame non-blank
- long-press → edit mode / wiggle / badges
- drag lift+scale+follow / reorder / empty-cell drop / rightmost columns / edge autoscroll
- Done / Esc / empty click / settings gear exit
- delete badge hide
- reorder persistence across restart / hidden persistence across restart(目標。foreground ロックで厳しければ deterministic test + キャプチャできる範囲で検証し、未確認項目を明記)

### 完了報告内容
変更概要 / behavior-preserving の根拠 / 残した adapter / 実行テスト / `codex review --base main` 結果 / screen verification checklist / 未確認項目 / PR 作成

### リスクと方針遵守
- 範囲が広くなったら実装前に分割案を出す → 今回は 1 PR・6 コミットで構造化。
- `scroll.rs` / search / bottom-control 状態機械 / settings / click passthrough は触らない。
- `LauncherItem` / folder domain / 全体 `AppAction`/`AppCommand` / renderer facade 分割は次 Phase 以降。
- stable `AppId` 基準を維持。index を永続的意味として扱わない。