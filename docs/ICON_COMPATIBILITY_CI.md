# アイコン互換性CI

`.github/workflows/macos-icon-compatibility.yml` は、macOSのバージョン差によって
アプリアイコンの取得結果や外形が変わっていないかを確認するためのGitHub Actionsです。

## 何を検証するか

現在は、次の3種類のmacOSランナーで同じアプリのアイコンを取得します。

| ランナー | 役割 |
| --- | --- |
| `macos-14` | 比較の基準 |
| `macos-15` | macOS 15の比較対象 |
| `macos-26` | macOS 26の比較対象 |

対象アプリは、現在の代表例として次の2つです。

- App Store
- Activity Monitor

各ランナーで、本番アプリと同等のアイコン取得経路を使って次の2枚を保存します。

- `source.png` — 取得直後の画像
- `normalized.png` — アプリで表示する前の正規化後画像

比較はPNGファイルのバイト列ではなく、PNGを展開したRGBA画素で行います。これにより、
PNGメタデータや圧縮方法の違いではなく、実際に表示される画素の違いを検出できます。

## 何が結果として得られるか

比較ジョブは、macOS 14を基準にmacOS 15・26との差分を次の2つの観点で出力します。

### RGBA画素比較

取得直後と正規化後について、次の値を出します。

- RGBA画素が完全一致したか
- 変更された画素数
- 比較対象の総画素数

macOS側のデザイン変更を把握するための情報比較なので、差分があってもこの比較ジョブ自体は失敗扱いになりません。画像が存在しない、壊れている、または比較処理を実行できない場合は失敗します。

### 外形比較

透明度からアルファマスクを作り、アイコンの外形を比較します。

- `alpha >= 11` — 半透明の縁や影を含む、見えている外縁
- `alpha >= 128` — ほぼ不透明な本体部分
- 外接矩形 — `幅×高さ@(x,y)`
- マスク差分 — 基準と比較対象で所属が変わったピクセル数

外形比較画像はアプリごとに分かれ、列は常に `macOS 14 / macOS 15 / macOS 26` です。

カラー比較画像の行は次のとおりです。

- `SOURCE`
- `NORMALIZED`

外形比較画像の行は次のとおりです。

- `SOURCE MASK`
- `SOURCE DIFF`
- `NORMALIZED MASK`
- `NORMALIZED DIFF`

差分画像では、基準だけにある部分を青、比較対象だけにある部分をオレンジ、共通部分を薄いグレーで表示します。

## 起動条件

### PRの変更による自動実行

次のファイルに変更があるPRでは、自動的に実行されます。

- `Cargo.toml`
- `Cargo.lock`
- `src/icons/**`
- `src/platform/macos/**`
- `examples/macos_icon_capture.rs`
- `examples/macos_icon_compare.rs`
- `.github/workflows/macos-icon-compatibility.yml`

### ラベルによる再実行

PRに次のラベルを付けると、アイコン互換性CIを起動できます。

```text
icon-compatibility:macos
```

ラベル名はプラットフォームを末尾に置く形式にしています。将来Windows版を追加する場合は、例えば次の名前にできます。

```text
icon-compatibility:windows
```

同じPRで再実行したい場合は、ラベルを一度外してから付け直します。ラベルの追加イベントを起点に実行するためです。

### Actions画面からの手動実行

`workflow_dispatch`にも対応しているため、GitHubのActions画面で `macOS Icon Compatibility` を選び、`Run workflow`から起動できます。

## PR上で確認できるもの

比較ジョブが成功すると、PRに次の内容を含むコメントが1つ更新されます。

- アプリごとのRGBA比較結果
- アプリごとの外形比較結果
- GitHub Actionsの実行番号・Run ID・検証対象コミットへのリンク
- macOS 14 / 15 / 26の取得Artifactへのリンク
- Markdown / JSONの比較レポートへのリンク
- アプリ別カラー比較画像
- アプリ別外形比較画像

比較レポートArtifactには、少なくとも次のファイルが含まれます。

```text
compatibility-report.md
compatibility-report.json
compatibility-preview-activity-monitor.png
compatibility-preview-app-store.png
compatibility-shape-preview-activity-monitor.png
compatibility-shape-preview-app-store.png
```

取得元の画像はOSごとのArtifactに入り、各アプリのディレクトリに次の構成で保存されます。

```text
macos-14/<app>/source.png
macos-14/<app>/normalized.png
```

`macos-15`と`macos-26`も同じ構成です。

## ローカルでの比較

3つのOSで取得した画像を同じディレクトリに置いたあと、比較処理だけをローカルで実行できます。

```bash
cargo run --locked --example macos_icon_compare -- \
  --root target/macos-captures \
  --baseline macos-14 \
  --report target/macos-captures/compatibility-report.md \
  --preview target/macos-captures/compatibility-preview.png \
  --shape-preview target/macos-captures/compatibility-shape-preview.png
```

実行すると、指定したベース名からアプリ別のPNGが作られます。例えば、
`compatibility-preview.png`を指定した場合は次のファイルが生成されます。

```text
compatibility-preview-activity-monitor.png
compatibility-preview-app-store.png
compatibility-shape-preview-activity-monitor.png
compatibility-shape-preview-app-store.png
```

## 解釈上の注意

- macOS 14が比較基準であり、macOS 15・26の差分を評価します。
- RGBA差分があること自体は、アイコンデザインやOS側の合成方法が変わった可能性を示す情報です。
- 外形の差分が小さくても、線の太さ、影、半透明の縁、色の変更はRGBA比較で差分になります。
- macOSランナーのイメージ更新によって、同じmacOS番号でも取得結果が変わる可能性があります。PRコメントのランナー番号と取得Artifactを併せて確認します。
