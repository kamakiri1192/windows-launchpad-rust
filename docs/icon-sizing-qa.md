# Icon Sizing Visual QA — Issue #48

Issue #48（自由形アイコンの視覚的サイズ自動正規化）の Visual QA 手順。

## 前提

- ブランチ `feat/icon-visual-sizing-48`
- 実装: `src/icons/sizing.rs`（分類）+ `src/icons/normalize.rs`（スケール適用）+ `src/icon_cache.rs`（記録）
- `SCHEMA_VERSION` 1→2、`EXTRACTION_VERSION` バンプ済み → 初回起動で全キャッシュ再抽出

## 1. ビルド確認

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features
cargo build --release
```

全て green であることを確認（実装時点で 838 tests passed, clippy clean）。

## 2. Rust 検証ツール（`icon_audit`）

`examples/icon_audit.rs` は macOS 専用。実際のアプリアイコンに対して Rust 実装の
`sizing` + `normalize` を適用し、分類結果とスケール適用結果を出力する。

```sh
cargo run --release --example icon_audit -- --out /tmp/icon-audit
```

生成物:
- `icon-audit-report.json`: 全アイコンの分類・スケール・solid_fill
- `icon-audit-fullbleed.png`: FullBleed アイコンのコンタクトシート
- `icon-audit-thinline.png`: ThinLine アイコンのコンタクトシート
- `icon-audit-solid.png`: Solid アイコンのコンタクトシート
- `icon-audit-compare.png`: ThinLine/Solid の [現状 | 提案] 比較

確認ポイント:
- FullBleed シートの Safari / App Store / Notes 等が現状と変わらないこと
- ThinLine シートの MIDI Monitor / SynthV / Webcam が scale=0.92 で自然なこと
- Solid シートの VLC / Unity Hub / Cinema 4D / Barrier が scale=0.74 で自然なこと
- compare シートの左右で Logo 系が適度に縮小されていること

## 3. 実機 Visual QA（macOS）

> **注意**: Liquid Glass 表現のため、通常起動ではスクリーンショットが無効化
> されている（`docs/EDIT_MODE_VISUAL_QA.md` 参照）。キャプチャには
> `LAUNCHPAD_ALLOW_SCREENSHOT=1` が必要。

### 3.1 初回起動（Cold cache）

`SCHEMA_VERSION` バンプにより既存キャッシュが全件再抽出される。

```sh
# 既存キャッシュのバックアップ（既に実施済みなら skip）
cp ~/Library/Application\ Support/Launchpad/cache.sqlite3 \
   ~/Library/Application\ Support/Launchpad/cache.backup-$(date +%Y%m%d).sqlite3

# キャッシュ削除（強制的に cold cache にする）
rm ~/Library/Application\ Support/Launchpad/cache.sqlite3*

# キャプチャ有効 + release build 起動
LAUNCHPAD_ALLOW_SCREENSHOT=1 cargo run --release
```

確認:
- [ ] 起動時のアイコン読込が正常（抽出が走る、数秒待つ）
- [ ] FullBleed アイコン（Safari, App Store 等）が現状と同じサイズで表示
- [ ] Logo 系アイコン（VLC, Unity Hub, Cinema 4D 等）が適度に縮小されて表示
- [ ] ThinLine 系（MIDI Monitor, SynthV 等）が潰れずに自然なサイズで表示
- [ ] タイル角丸でアイコンが切れていない（VLC 等の縦長ロゴ）

### 3.2 Warm cache 確認

一度終了し、再度起動（キャッシュあり）:

```sh
LAUNCHPAD_ALLOW_SCREENSHOT=1 cargo run --release
```

確認:
- [ ] Cold cache 時と見た目が完全に一致する（キャッシュから正しく読込まれる）
- [ ] 起動が高速（抽出が走らない）

### 3.3 スクリーンショット

macOS では `LAUNCHPAD_ALLOW_SCREENSHOT=1` でキャプチャ可能。
`cmd+shift+4` でランチャー全体をキャプチャし、PR に添付。

## 4. DB 内容確認

キャッシュに category と scale が記録されているか確認:

```sh
sqlite3 ~/Library/Application\ Support/Launchpad/cache.sqlite3 \
  "SELECT display_name, category, scale, image_w, image_h FROM icons ORDER BY category, scale LIMIT 30;"
```

確認:
- [ ] category 列に `fullbleed` / `thinline` / `solid` が格納されている
- [ ] scale 列に 1.0 / 0.92 / 0.74 が格納されている
- [ ] FullBleed は scale=1.0、ThinLine は scale=0.92、Solid は scale=0.74

## 5. 受け入れ基準チェック

- [ ] 自由形アイコンがタイルいっぱいに膨らみすぎない（Solid 0.74 / ThinLine 0.92）
- [ ] 小さく見えるアイコンが過度に縮んだままにならない（FullBleed 不変、Logo 適正）
- [ ] 既存の四角いアイコンの見た目が不自然に変わらない（FullBleed scale=1.0）
- [ ] 固定 atlas / cache と矛盾しない（128px セル、CELL=130 そのまま）
- [ ] cargo test / clippy / build --release 全 green
- [ ] cold/warm cache で見た目一致
- [ ] DB schema version 2 で category/scale が記録される
- [ ] 比較スクリーンショット取得済み

## 6. 既知の制限

- FileZilla（α形状だが solid_fill=0.94）は FullBleed 判定になる（自動判別困難、将来の手動指定で対応）
- macOS 専用の検証ツール（Windows は別途 hand-testing が必要）