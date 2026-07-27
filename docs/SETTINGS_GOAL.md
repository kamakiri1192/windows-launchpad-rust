# Settings Panel — Goal Prompt (Issue #33 follow-up)

> このファイルは別セッションで設定パネル本体を実装するための引き継ぎプロンプトです。
> ステップ1（設定入口＝編集モードのギア＋プレースホルダパネル）は PR #64 で完了済み。

## 目標

Issue #33 の残り：Mac 型サイドバー設定パネル本体を実装する。
プレースホルダパネル（画面中央の「設定」ガラス＋×閉じる）を、本物の 2 ペイン設定 UI に置き換える。

## レイアウト方針（ユーザ承認済み）

**Mac 型サイドバー**（macOS Ventura 環境設定風）。

- 左ペイン: カテゴリ一覧（アプリ／表示・検索／システム／について）。
- 右ペイン: 選択中カテゴリの設定行。
- 両ペインともガラス背景。現在の `render_settings_panel` の 1 枚ガラスの上に、区切り線＋2 領域を描画する形で拡張。
- 行高はゆったり（iOS 56pt 相当）に取り、Mac 感と「最小セットでも寂しくない」バランスを出す。
- 空きスペースには macOS 風の補足テキストを添える。

## v1 設定項目（最小セット — ユーザ承認済み）

| カテゴリ | 項目 | コントロール | 関連 issue |
|---|---|---|---|
| アプリ | 並び順 | 単一選択（名前順 / 手動 / 最近使った / よく使う） | #34 |
| アプリ | 非表示アプリ | chevron（サブページ or 数値表示） | #35 |
| アプリ | よく使うアプリ | トグルスイッチ | #36 |
| 表示・検索 | 検索時に非表示アプリを含める | トグルスイッチ | #35 |
| システム | キャッシュをリセット | アクション行（chevron） | — |
| システム | 設定をリセット | アクション行（chevron） | — |
| について | バージョン | テキスト | — |

> 各機能の**実本体**は関連 issue に委ね、v1 の設定 UI は「フック＋トグル／選択の保持」のみ。トグルを切り替えても即座に挙動が変わらなくてもよい（永続化までで OK）。

## 永続化（serde 導入）

プロジェクト初の serde 導入。既存 SQLite `kv` テーブルを再利用。

```rust
// src/settings.rs（新規）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub sort_order: SortOrder,        // #34
    pub frequent_apps_enabled: bool,  // #36
    pub search_includes_hidden: bool, // #35
    // 拡張余地
}
```

- 保存先: `IconCache` の `kv_get`/`kv_put`（`src/icon_cache.rs:320-339`）に `SETTINGS_KEY = "settings"` を追加。
- `Settings` を `serde_json` で `Vec<u8>` 化して `kv_put`。
- 既存パターンを踏襲: `load_customization`（`src/main.rs` 起動時 1 回）＋ `persist_*`（変更時）。`app_order`/`hidden_ids` と同じ。
- `Cargo.toml` に `serde` + `serde_json` 追加（features = ["derive"]）。

## すでに完成している共有基盤（PR #64 — 作り直さないこと）

以下は実装済み。設定パネル本体を作る上で再利用・拡張する：

- **状態**: `App.settings_open: bool`（`src/main.rs` の App 構造体）。`open_settings()`/`close_settings()`/`toggle_settings()` メソッド。
- **入口**: 編集モードの `[完了][⚙]` のギアクリック → `open_settings()`。トレイ右クリック「設定」→ `UserEvent::ToggleSettings` → `summon()`＋`toggle_settings()`。
- **Esc 優先チェーン**: `settings_open` が最優先（`src/main.rs` の `KeyboardInput` ハンドラの冒頭）。
- **フォーカス喪失ガード**: `settings_open` 中は `Focused(false)` で隠れない。
- **再描画維持**: `settings_open` を redraw 条件に OR 済み（RedrawRequested 末尾＋`about_to_wait`）。
- **パネル描画基盤**: `render_settings_panel()`（`src/main.rs`）。ガラスパネル形状（`set_settings_panel_glass_shape`）＋インク（`set_settings_instances`：×閉じる）＋テキスト（`set_settings_text_instances`：タイトル）。これを 2 ペイン化する。
- **パネル判定**: `settings_panel_center()`, `settings_panel_half()`, `settings_panel_contains()`, `settings_panel_hit_close()`（`src/main.rs`）。
- **外クリックで閉じる**: `MouseInput` の `settings_open` ブロック。パネル外クリック → `close_settings()`。
- **GPU plumbing**: `gear_shape`/`settings_panel_shape` は共に独立ガラスパス済み。サイドバーの区切り線や行は `ControlInstance`/テキストクワッドで追加可能。

## renderer 側の構造（設定 UI 拡張の着地点）

- `src/renderer.rs`: `set_settings_instances`（インク：トグル、チェック、chevron などの SDF）＋ `set_settings_text_instances`（行ラベル、セクションヘッダ）。
- `src/shader_control.wgsl`: トグルスイッチ（KIND_TOGGLE）、チェックマーク（KIND_CHECK）、chevron（KIND_CHEVRON）を追加。既存の `KIND_*` 定数は `src/bottom_control.rs` にあり、shader の `fs_main` が kind で分岐。トグルは `kind > 3.5 && kind < 4.5` のスクロール/フレームマスク条件に引っ掛からないよう注意（ギア=5 と同様）。
- `src/liquid_glass/renderer.rs`: `render_settings_panel` で 1 枚ガラスを描画中。サイドバーと詳細ペインで別々のガラスにするなら、更に形状バッファを増やすか、1 枚ガラス上に区切り線を描く（後者が安い）。

## スクロール

設定項目が増えた場合、右ペインがスクロール必要。既存 `src/scroll.rs` はグリッドの水平 1 本用。垂直スクロールは新規だが、physics（ドラッグ＋慣性＋スナップ）は流用可能。

## 設定変更の既存 UI への反映

- 並び順（#34）: `registry.set_order` 相当＋`relayout()`。
- 非表示（#35）: 既存 `hidden_ids`（`kv` の `hidden_ids` キー）と連動。
- よく使う（#36）: 起動回数記録が未実装（#36 本体）。v1 はトグル保持のみ。

## 連動 issue

- #34 並び順, #35 非表示アプリ, #36 よく使うアプリ, #44 自動更新, #46 スクショモード。
- これらの実装と設定 UI のフックを繋ぐ。

## 受け入れ条件（issue #33 より）

- [x] 設定メニューを開ける。（PR #64 で達成）
- [ ] 設定を変更できる。
- [ ] 設定が再起動後も維持される。
- [ ] 設定変更が既存 UI と自然につながる。
- [ ] 設定リセットができる。
- [ ] `cargo test` / `cargo build --release` が通る。
- [ ] 設定中に Win+Space / Esc / トレイ操作が破綻しない。

## 注意事項

- 浮遊ギア（左下コーナー／ピル内）は一度作って破棄した。参考にしないこと。現行の入口は「編集モードのギア」のみ。
- `GearStyle` enum / `LAUNCHPAD_GEAR_STYLE` env var は削除済み。
- コーナーギアヘルパ（`gear_geometry`/`gear_instance` 等）も削除済み。編集モードギアは `edit_gear_*` 系。
- `KIND_CLOSE`/`KIND_GEAR` は `src/bottom_control.rs` で `pub`。新規 kind を足す場合はここに定義し、shader の `element_extent` と `fs_main` の分岐を更新する。

## Debug カテゴリの拡張（issue #112）

「キーボードショートカットでデバッグできることを、設定画面からも同様に」を
実現するため、Debug カテゴリをセクション見出し付きで大幅に拡張した。

### 配置（スクロール対応）

```
[トグル] 開発者ショートカットを有効化      (debug_keys_enabled)  ※永続化
── ウィンドウ ──
[トグル] ウィンドウ装飾                    (M)  ※セッションのみ
[ボタン] アイコンキャッシュを再構築        (R)
── Liquid Glass ──
[トグル] Liquid Glass を有効化             (V)  ※永続化
[スライダー+↺] thickness                   (1/2)  ※永続化
[スライダー+↺] refractive_index            (3/4)  ※永続化
[スライダー+↺] saturation                  (5/6)  ※永続化
[スライダー+↺] chromatic_aberration        (7/8)  ※永続化
[スライダー+↺] blur_radius                 (9/0)  ※永続化
[トグル] 色収差を無効化                    (C)  ※セッションのみ
[トグル] エッジライティングを無効化        (E)  ※セッションのみ
[トグル] ブラーを無効化                    (L)  ※セッションのみ
[ボタン] Liquid Glass をデフォルトに戻す   ← 一括リセット（V＋数値5種）
── デバッグビュー ──
[トグル] backdrop texture                  (B)  ※セッションのみ
[トグル] geometry texture                  (G)
[トグル] displacement                      (D)
[トグル] alpha mask                        (A)
[トグル] final glass only                  (F)
```

### 永続化の設計

- **永続化対象**: `domain::settings::LiquidGlassSettings`（`enabled` ＋ 数値5種）。
  `#[serde(default)]` で Settings に統合し、古い JSON からの前方互換デコードに対応。
- **セッションのみ**: ウィンドウ装飾(M)、デバッグビュー(B/G/D/A/F)、無効化フラグ(C/E/L)。
  これらは保存しないことで、誤ってデバッグ状態を残したまま終了しても影響を残さない。
- キーボードでパラメータを変更した場合も、`LiquidGlassKey` ハンドラが
  renderer の params を `settings.liquid_glass` へ同期してから persist する
  （`src/app/action.rs::handle_keyboard` の `LiquidGlassKey` 分岐）。

### UI 部品の新規追加

- `ControlKind::SliderTrack` / `SliderKnob` / `ResetIcon`（kind 10/11/12）。
  `shader_control.wgsl` の `element_extent` / `fs_main` に分支を追加。
- スライダーのジオメトリ計算は `liquid_glass_studio.rs` から**コピー**
  して `src/layout/settings_panel.rs` に実装（後続PRで本体LG部品を書き換える際、
  共通化しないため）。`debug_slider_geometry` / `debug_slider_value_from_pointer`
  / `debug_classify_row_hit` が中心。
- 個別リセットは各行の ↺ アイコン、一括リセットはセクション末尾のボタン。

### スクロール

- Debug カテゴリは内容がパネル高さを超えるため、**行単位の縦スクロール**を
  実装した（`settings_scroll_rows: i32`）。ホイール/トラックパッドは
  `WindowEvent::MouseWheel` を新規ハンドル（`src/app/handler.rs`）。
- クリップ機能（テキスト/インクの矩形マスク）は RenderModel に無いため、
  **表示範囲外の行は生成時にフィルタ** して描画しない方式（`debug_row_is_visible`）。
  ピクセル単位の滑らかなスクロールは今後の課題。

### Renderer API

`LiquidGlassRenderer` に getter/setter を追加（`params()` / `debug_options()` /
`set_thickness()` 等 / `reset_params_to_defaults()` / `toggle_debug_flag()`）。
`LiquidGlassParams` / `DebugOptions` の構造体自体には**触れない**
（後続PRで置き換えるため）。`domain` と `liquid_glass` の循環依存を避けるため、
renderer 側に `SettingsDebugFlag` のミラー enum を持つ。
