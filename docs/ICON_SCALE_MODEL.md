# アイコンの視覚サイズ学習モデル

この機能は、ランチャーに並ぶアプリアイコンの「見た目の大きさ」をそろえるためのものです。

現在の `solid_fill` による3段階判定を基準として残しつつ、人間が調整した倍率から補正傾向を学習します。学習・推論はすべてローカルで完結し、Python、ONNX Runtime、Core ML、DirectML、GPU、NPUは不要です。

## 仕組み

新しく抽出したアイコンでは、次の優先順位で倍率を決めます。

1. アプリIDまたは元パスに対する個別上書き
2. 学習済みGradient Boostingモデル
3. 既存の `solid_fill` ルール（`1.00`、`0.92`、`0.74`）

最終倍率は `0.55..=1.10` に制限されます。

モデルが壊れている、形式が古い、学習件数が足りない、特徴量が学習範囲から外れている、交差検証の精度が低い、といった場合は安全に既存ルールへ戻ります。

推論はアイコン抽出時に1回だけ実行されます。結果は128×128のRGBA画像へ焼き込んでSQLiteへ保存するため、描画やアニメーション中にモデル処理は発生しません。

---

# 使い方

## 0. 作業ブランチへ移動する

PRのブランチを使う場合は、リポジトリのルートで次を実行します。

```bash
git fetch origin
git switch agent/icon-visual-features
```

依存関係を取得できる状態であることを確認してください。

```bash
cargo check
```

## 1. 校正用ワークスペースを生成する

### macOS

```bash
cargo run --example icon_scale_audit -- \
  --out-dir ./icon-scale-audit
```

### Windows PowerShell

```powershell
cargo run --example icon_scale_audit -- `
  --out-dir .\icon-scale-audit
```

実行すると、インストール済みアプリを走査して次のファイルを生成します。

```text
icon-scale-audit/
├── audit.json
├── calibrate.html
└── icons/
    ├── 00000.png
    ├── 00001.png
    └── ...
```

- `audit.json`
  - 19個の特徴量
  - 現在のルール倍率
  - アプリIDや元パス
- `icons/*.png`
  - 現在のルールで正規化したプレビュー
- `calibrate.html`
  - オフラインで動く校正画面

## 2. 校正画面を開く

### macOS

```bash
open ./icon-scale-audit/calibrate.html
```

### Windows PowerShell

```powershell
Start-Process .\icon-scale-audit\calibrate.html
```

校正画面の基準アイコンと各カードは、ランチャーと同じ84×84論理pxのタイルと、タイルより18px大きいガラスハローで表示します。アプリ内部の正規化画像は128×128pxですが、実際の描画ではタイルの物理サイズへサンプリングされます。

RetinaやWindowsの高DPI環境では、アプリの84×DPI物理pxに対して、ブラウザの84 CSS pxがdevicePixelRatioに応じて物理pxへ描画されます。校正時に84pxへDPI倍率を手動で掛けないでください。ブラウザの表示倍率は100%にしてください。

### 比較基準の選び方

比較基準には、次のようなアイコンを選ぶと調整しやすくなります。

- 透明余白が極端に多くない
- 一般的な角丸四角形
- 線が細すぎない
- 色が極端に薄くない
- 一覧の中で自然な大きさに見える

基準アイコン自体の輪郭へ機械的に合わせるのではなく、一覧で見たときの次の印象をそろえます。

- 面積
- 線の太さ
- 色の強さ
- 中央の密度
- 全体の存在感

### 校正操作

1. 上部の「比較基準」で基準アイコンを選ぶ
2. 各カードのスライダーを動かす
3. 基準と同じ程度の存在感に見えたら「この倍率で確定」をオンにする
4. 最低12件、できれば50件以上を確定する
5. 「labels.jsonを書き出す」を押す

校正途中の内容はブラウザのLocal Storageへ保存されます。同じ場所の `calibrate.html` を開けば続きから再開できます。

学習データには、できるだけ形状の異なるアイコンを含めてください。

- 角丸四角形
- 円形
- 細線ロゴ
- 自由形状
- 複数パーツ
- 中抜き形状
- 暗いアイコン
- 薄い色のアイコン

## 3. `labels.json` の場所を確認する

ブラウザから書き出した `labels.json` は、通常はダウンロードフォルダーへ保存されます。

### macOSの例

```text
~/Downloads/labels.json
```

### Windowsの例

```text
$HOME\Downloads\labels.json
```

`icon-scale-audit`フォルダーへ移動しても構いません。

### macOS

```bash
mv ~/Downloads/labels.json ./icon-scale-audit/labels.json
```

### Windows PowerShell

```powershell
Move-Item $HOME\Downloads\labels.json .\icon-scale-audit\labels.json
```

## 4. 学習してランチャーへ導入する

### macOS

```bash
cargo run --example icon_scale_train -- \
  --audit ./icon-scale-audit/audit.json \
  --labels ./icon-scale-audit/labels.json \
  --install
```

### Windows PowerShell

```powershell
cargo run --example icon_scale_train -- `
  --audit .\icon-scale-audit\audit.json `
  --labels .\icon-scale-audit\labels.json `
  --install
```

学習すると、既定では次のファイルもリポジトリ直下へ出力します。

```text
icon-scale-model/
├── icon-scale-model.json
├── icon-scale-overrides.json
└── training-report.json
```

- `icon-scale-model.json`
  - 学習済みの決定株モデル
- `icon-scale-overrides.json`
  - 確定済みアイコンの正確な手動倍率
- `training-report.json`
  - 学習件数
  - 決定株の数
  - 学習データ上のRMSE
  - 交差検証RMSE
  - 個別上書きの件数

`--install`を付けると、次の場所にもコピーされます。

### macOS

```text
~/Library/Application Support/Launchpad/
```

### Windows

```text
%LOCALAPPDATA%\Launchpad\
```

## 5. ランチャーを完全終了して再起動する

モデルはアイコンワーカー起動時に読み込まれます。

### macOSの例

```bash
pkill -f launchpad-windows
```

その後、通常の方法でランチャーを起動してください。

モデルまたは個別上書きが変わると、保存済みの128×128アイコンキャッシュを一度だけ削除し、新しい倍率で再抽出します。

キャッシュ削除に失敗した場合はリビジョンを更新しないため、次回起動時に再試行します。

---

# モデルだけの性能を確認する

通常の学習では、確定したアイコンに対して個別上書きも作成します。

そのため、確定済みアイコンはモデル予測ではなく手動倍率がそのまま使われます。モデル単体の性能を確認したい場合は `--no-overrides` を付けます。

### macOS

```bash
cargo run --example icon_scale_train -- \
  --audit ./icon-scale-audit/audit.json \
  --labels ./icon-scale-audit/labels.json \
  --no-overrides \
  --install
```

### Windows PowerShell

```powershell
cargo run --example icon_scale_train -- `
  --audit .\icon-scale-audit\audit.json `
  --labels .\icon-scale-audit\labels.json `
  --no-overrides `
  --install
```

この場合も空の有効な `icon-scale-overrides.json` を導入します。以前の個別上書きが残って、モデル検証を妨げることはありません。

## 推奨する確認手順

1. まず `--no-overrides --install` でモデルだけを導入する
2. ランチャーを再起動する
3. 未学習アイコンを含め、一覧全体のサイズ感を確認する
4. 問題がなければ通常の `--install` で個別上書きも導入する

---

# 精度の見方

`training-report.json`には2種類のRMSEがあります。

## `training_rmse_log_scale`

学習に使用したアイコン自身に対する誤差です。

小さくても、同じデータを暗記しているだけの可能性があります。

## `validation_rmse_log_scale`

データを複数グループへ分け、一部を学習から外して予測した交差検証誤差です。

ランタイムの信頼度判定には、こちらを使用します。

値は対数倍率の誤差です。おおよその倍率差は次で確認できます。

```text
倍率差 ≈ exp(RMSE) - 1
```

例：

```text
RMSE 0.03 → 約3.0%
RMSE 0.05 → 約5.1%
RMSE 0.10 → 約10.5%
RMSE 0.18 → 約19.7%
```

交差検証RMSEが大きいモデルは、ランタイムで信頼度が下がります。信頼度が `0.35` 未満の場合はモデルを使わず、既存ルールへ戻ります。

信頼度が十分でも、確信度が低いモデルほど補正量を弱めます。既存ルールを基準にした安全側の動作です。

---

# リセット方法

学習機能を完全に無効化して、従来の3段階ルールへ戻す場合は、次の2ファイルを削除してランチャーを再起動します。

```text
icon-scale-model.json
icon-scale-overrides.json
```

### macOS

```bash
rm -f \
  ~/Library/Application\ Support/Launchpad/icon-scale-model.json \
  ~/Library/Application\ Support/Launchpad/icon-scale-overrides.json
```

### Windows PowerShell

```powershell
Remove-Item -ErrorAction SilentlyContinue `
  "$env:LOCALAPPDATA\Launchpad\icon-scale-model.json", `
  "$env:LOCALAPPDATA\Launchpad\icon-scale-overrides.json"
```

再起動時にポリシー変更を検出し、アイコンキャッシュを一度だけ再生成します。SQLiteを手作業で編集する必要はありません。

---

# トラブルシューティング

## `training failed: ... at least 12 are required`

確定済みアイコンが12件未満です。

`calibrate.html`を開き、最低12件を確定してから `labels.json` を再度書き出してください。実用上は50件以上を推奨します。

## `failed to read ... labels.json`

ブラウザから書き出したファイルが、コマンドで指定した場所にありません。

ダウンロードフォルダーの実際のパスを指定するか、`icon-scale-audit`へ移動してください。

## 導入したのに表示が変わらない

次を確認してください。

1. `--install`を付けたか
2. ランチャーを完全終了したか
3. モデルファイルがアプリケーションデータフォルダーにあるか
4. 起動ログに `icon-scale: loaded model=true` が出ているか
5. `--no-overrides`の空ファイルが意図せず残っていないか

## 確定したアイコンだけ完璧で、未確定アイコンが不自然

個別上書きは正しく機能していますが、モデルの汎化性能が不足しています。

- 校正件数を増やす
- 異なる形状を追加する
- `training-report.json`の交差検証RMSEを確認する
- `--no-overrides`でモデルだけの結果を確認する

## 元へ戻したのに一部アイコンが変わらない

ランチャーがまだ動いている可能性があります。完全終了してから再起動してください。

キャッシュ削除に失敗した場合はリビジョンが更新されないため、次回起動で再試行されます。

---

# 技術概要

## 学習対象

モデルは、現在のルール倍率に対する補正を学習します。

```text
log(manual_scale / rule_scale)
```

最終倍率そのものを直接覚えるのではなく、既存ルールからどれだけ増減すべきかを学習します。

## モデル

純Rustで実装したGradient Boostingです。

各決定株は、19個の特徴量のうち1つを閾値と比較して、小さな補正値を加えます。通常は100本未満で、推論は単純な浮動小数点演算だけです。

## 交差検証

データ件数に応じて3～5分割の決定的な交差検証を行います。

- 12～23件：3分割
- 24～49件：4分割
- 50件以上：5分割

その後、全データでもう一度最終モデルを学習します。

## 特徴量

固定順序の19特徴量を使用します。

- 4段階のα占有率
- α質量
- 外接矩形の幅、高さ、面積
- 対数縦横比
- 既存の `solid_fill`
- α加重重心
- 輪郭比率と円形度
- 中抜き率
- 連結成分数と最大成分比率
- α加重輝度の平均と標準偏差

モデルには特徴量名と順序も保存します。ビルド側と一致しないモデルは読み込みません。

## 実行性能

特徴量抽出と推論は既存のバックグラウンドアイコンワーカーで実行します。

MシリーズMacやCopilot+ PCでは、CPUだけで十分に軽量です。GPUやNPUの導入による効果より、ランタイムと配布の複雑化の方が大きいため使用しません。
