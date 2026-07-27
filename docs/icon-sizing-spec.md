# Issue #48 — 自由形アイコンの視覚的サイズ自動正規化 詳細仕様

関連: https://github.com/kamakiri1192/windows-launchpad-rust/issues/48
参照: `docs/icon-guide-research.md`（Apple HIG グリッドのランチャー向け解釈）
ブランチ: `feat/icon-visual-sizing-48`

## 1. 目的と設計思想

Apple HIG（research §15）の核心: **「統一すべきは外接矩形の寸法ではなく、見かけの面積と視覚的な重さである」**。

現状の正規化（`src/icons/normalize.rs`）は「透明余白を crop → 長辺を 128px に fit」のみで、面積も縦横比も無視している。その結果:
- **フルブリード型**（Safari, App Store, メモ）: 128px セル全面を埋め、相対的に主張しすぎる。→ **現状維持（縮小しない）**。
- **自由形状型**（Unity, VLC, MIDI Monitor 等）: 縦長・面積小さく、周囲より貧弱に見える。→ **Apple 基準（70-78%）に縮小し、視覚的重さを揃える**。分類は `solid_fill` で行う（§3 参照）。

本仕様は **正規化ステップ（`normalize_to`）で分類＋面積解析を行い、ピクセルを最終表示サイズに焼き込む** 方針（ユーザ承認）。シェーダ・インスタンス構造・アトラスレイアウトは**変更しない**。

## 2. 方針決定（原画像検証で確定 2026-07-28）

3 カテゴリ分類（`solid_fill` = bbox 内 alpha>=128 充填度、で判定）:

```
solid_fill >= 0.75  →  FullBleed   (scale = 1.00, 不変)
solid_fill <  0.40  →  ThinLine    (scale = 0.92, 細線画・控えめ縮小)
それ以外(0.40-0.75) →  Solid       (scale = 0.74, Apple §6.2 準拠)
```

決定根拠（原画像 259 アプリ実測）:

- `solid_fill` は macOS squircle（Safari, App Store）を正しく FullBleed 分類
- ThinLine は MIDI Monitor, Synthesizer V, Webcam, Civ6 等の細線画ロゴを捕捉
- Solid は VLC, Unity, Cinema 4D 等の実体あるロゴ
- 実測分布: FullBleed 200 / ThinLine 7 / Solid 52

検証済みパラメータ表（実測値、ユーザ確認済み）:

| アプリ | solid_fill | 判定 | scale | ユーザ評価 |
|---|---|---|---|---|
| Safari, App Store, Notes | 0.85 | FullBleed | 1.00 | ✓ 不変 |
| sbv2-gui, Remote Codetrol, TouchOSC | 1.00 | FullBleed | 1.00 | ✓ 不変 |
| MIDI Monitor | 0.31 | ThinLine | 0.92 | ✓ |
| Synthesizer V | 0.37 | ThinLine | 0.92 | ✓ |
| VLC, Unity Hub, Cinema 4D, Barrier | 0.50-0.73 | Solid | 0.74 | ✓ 完璧 |

既知の制限（合意済み）: FileZilla（solid_fill=0.94、α 形状）は FullBleed 判定になる。「α 形状だが高充填」の自動判別は困難。将来の手動指定機能で対応。

決定事項一覧表:

| 決定項目 | 採用 |
|---|---|
| 計算タイミング | 正規化時に焼き込み |
| 分類指標 | solid_fill のみ |
| スケール値 | 1.00 / 0.92 / 0.74 |
| FileZilla | FullBleed 扱い（諦める） |
| アイコン抽出経路 | 触らない |
| DB 記録 | category + scale 追加 |

## 3. アイコン種別の定義と判定

解析は **crop 前（原寸ソース画像）** に対して行う。

### 3.1 測定量

- `alpha >= 128` (`ALPHA_HARD`) のピクセルの外接矩形 `bbox`
- `bbox_w`, `bbox_h`
- `solid_area` = alpha >= 128 のピクセル数
- `solid_fill` = `solid_area / (bbox_w * bbox_h)`  ※ bbox 内の充填度
- `ALPHA_HARD = 128`（ソフト影・アンチエイリアス辺縁を除外する閾値）

### 3.2 分類ルール（確定）

```
if solid_fill >= 0.75:  FullBleed
elif solid_fill < 0.40: ThinLine
else:                   Solid
```

**核心**: macOS squircle（Safari 等）は角が大きく丸く透明のため、四辺接触率は全て 0.000 になる（「四辺接触→FullBleed」は成立しない）。しかし bbox 内は 85% 以上が不透明（背景が塗られている）なので `solid_fill` は 0.85 程度になり、Logo 系（0.31-0.73）と明確に分離する。

### 3.3 検証データ（原画像 259 アプリ）

| カテゴリ | solid_fill 範囲 | 代表例 | 件数 |
|---|---|---|---|
| FullBleed | 0.75 - 1.00 | Safari(0.853), sbv2-gui(1.000), TouchOSC(1.000) | 200 |
| Solid | 0.40 - 0.75 | VLC(0.546), Unity Hub(0.725), Barrier(0.503) | 52 |
| ThinLine | 0.27 - 0.40 | MIDI Monitor(0.311), SynthV(0.369), Civ6(0.273) | 7 |

## 4. スケール計算アルゴリズム（簡素化・確定）

各カテゴリの `scale` は固定値:

- FullBleed: `scale = 1.00`（現行 normalize と完全同等、不変）
- ThinLine: `scale = 0.92`（細線画保護。Apple §10.1「1px 未満の線は消失」への配慮）
- Solid: `scale = 0.74`（Apple §6.2「自由形状 最大寸法 70-78%」の中間値）

`scale` の定義: 「bbox 長辺 = S（128px）」を `scale = 1.0` とする。

- `scale = 0.74` → bbox 長辺 = 0.74 × 128 = 95px に配置
- `scale = 0.92` → bbox 長辺 = 0.92 × 128 = 118px に配置

### 4.1 計算式（ソース解像度非依存）

```
base = S / max(bbox_w, bbox_h)    # bbox 長辺を S にする基本比率
applied = base * scale             # カテゴリ別 scale を適用
new_w = round(bbox_w * applied)
new_h = round(bbox_h * applied)
# 128x128 キャンバスに中央配置
dx = (S - new_w) / 2
dy = (S - new_h) / 2
```

配置: 幾何学中央配置（現行の `dx`, `dy` 計算を維持）。v1 では光学補正なし。

### 4.2 FullBleed の特別扱い（不変保証）

`scale = 1.0` で現行 normalize と完全同等。Safari, App Store 等は 1 ピクセルも変わらない。ただし現行は `crop_to_opaque_bounds` を全アイコンに適用するが、FullBleed も crop される点は現行同等（squircle の透明角が crop 対象）。実は FullBleed の bbox は squircle 内接矩形になり、これを S に fit すると現行と同じ結果になる。

## 5. 影響範囲と変更点

### 5.1 変更ファイル

| ファイル | 変更 |
|---|---|
| `src/icons/sizing.rs`（新規） | `solid_fill` 計算 + 分類 + `scale` 決定の pure 関数。`IconCategory` enum |
| `src/icons/normalize.rs` | `normalize_to` で sizing モジュールを呼び出し、crop→scale→中央配置を実装 |
| `src/icons/mod.rs` | `sizing` モジュールの `pub use` 追加 |
| `src/icon_cache.rs` | `CachedIcon` に `category`, `scale` 追加。`SCHEMA_VERSION` 1→2。`EXTRACTION_VERSION` バンプ（Win4→5, mac5→6） |
| `src/workers/icon_worker.rs` | normalize が返す情報（`category`, `scale`）をキャッシュ書込に渡す。※ normalize のシグネチャ変更に追随 |
| `examples/icon_audit.rs`（新規） | 検証ツール。`Cargo.toml` に `[[example]]` 追加 |

### 5.2 変更しないもの

- アイコン抽出経路（`src/icons/extract.rs`, `src/platform/macos/apps.rs`）— 一切触らない
- シェーダ（`src/shader_icon.wgsl`, `src/renderer/prepare.rs`, `IconInstance`）
- アトラス（`src/renderer/icon_atlas.rs`）— `CELL=130`, スロット方式そのまま
- グリッドレイアウト（`src/grid.rs`, `src/layout/grid.rs`）
- タイル角丸マスク（normalize で焼き込むので、シェーダ側のマスクはそのまま）

### 5.3 パラメータ定数（確定値・Visual QA で微調整可能性）

```rust
pub const ALPHA_HARD: u8 = 128;
pub const FULLBLEED_FILL: f64 = 0.75;
pub const THINLINE_FILL: f64 = 0.40;
pub const SCALE_FULLBLEED: f64 = 1.00;
pub const SCALE_THINLINE: f64 = 0.92;
pub const SCALE_SOLID: f64 = 0.74;
```

これらは `sizing.rs` に集約。`examples/icon_audit.rs` でパラメータ掃引可能。

## 6. DB 拡張

`CachedIcon` に 2 フィールド追加:

- `category`: `IconCategory`（enum、serde または整数エンコードで格納）
- `scale`: `f32`（実際に適用した scale 値、デバッグ・将来の手動上書き用）

`SCHEMA_VERSION` 1→2 で全キャッシュ再抽出（`check_schema_version` が `DELETE FROM icons` を実行）。
`icons` テーブルに `category TEXT`, `scale REAL` カラム追加。
`CacheProbe` には追加しない（正規化時に決まる値なので、プローブ＝スキャン情報には無関係）。

## 7. 検証戦略

- `examples/icon_audit.rs`: 全 `.app` から原画像取得→分類→スケール適用→コンタクトシート PNG 出力
- ユニットテスト（`sizing.rs`）: 合成画像で `classify` の回帰
- `normalize.rs` 既存テスト: FullBleed は現行と同等であることを保証（`scale=1.0` の出力が現行と同じ）
- `cargo test` 全通過 + QA scenario（`qa/*.json`）が影響受けないこと（QA fixture は 128px solid を直接 atlas 書き込みで normalize 未経由）

## 8. 実装フェーズ

1. `sizing.rs` 新規（pure 関数 + テスト）
2. `normalize.rs` 統合 + `icon_cache.rs` 拡張 + worker 更新 + `EXTRACTION_VERSION` バンプ
3. `examples/icon_audit.rs` 作成
4. `cargo build` / `cargo test` / `cargo clippy` + Visual QA
5. レビュー + PR

## 9. 受け入れ基準（Issue より）

- [ ] 自由形アイコンがタイルいっぱいに膨らみすぎない（Solid 0.74 / ThinLine 0.92 で縮小）
- [ ] 小さく見えるアイコンが過度に縮んだままにならない（FullBleed は不変、Logo 系は適正サイズ）
- [ ] 既存の四角いアイコンの見た目が不自然に変わらない（FullBleed scale=1.0 完全不変）
- [ ] 固定 atlas / cache と矛盾しない（128px セル、CELL=130 そのまま）
- [ ] 自由形、正方形、縦長・横長のアイコンで比較できる
- [ ] `cargo test` green, `cargo build --release` green, `cargo clippy` green
- [ ] cold/warm cache で見た目一致（`SCHEMA_VERSION`/`EXTRACTION_VERSION` バンプで再抽出）
- [ ] QA scenario が壊れない
- [ ] 比較スクリーンショット取得済み

## 10. リスクと緩和・将来拡張

### 10.1 リスクと緩和

| リスク | 緩和 |
|---|---|
| FullBleed 回帰（完成アイコンが縮む） | テスト保証（scale=1.0 完全不変の回帰テスト） |
| ThinLine 閾値で細線画が漏れる | 実測で 0.31-0.37、閾値 0.40 で安全マージン確保 |
| Solid が小さすぎる/大きすぎる | scale=0.74（Apple §6.2 70-78% の中間）で実測確認済み |
| Cold cache 初回抽出が重くなる | 解析は O(画素数) で軽量 |
| macOS と Win で EXTRACTION_VERSION 統一の影響 | 両プラットフォーム全件再抽出（安全側） |

### 10.2 将来拡張（v1 範囲外）

- ユーザ個別手動指定（FileZilla 等の例外、per-icon scale override）
- alpha/luminance 重心による光学センタリング（HIG §9）
- 不定形アイコン用の薄いプレート/背景の自動生成（Liquid Glass トーン）
