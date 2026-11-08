# PR #237: ガラス描画の計測と最適化

対象は `1ea720c524737eb65e31d3857ea5a8ff114bc057`。作業開始時にGitHubの
open/draft PR headと一致することを確認した。下部バーと歯車が同じ完成シーンを
参照する修正を含む。計測後に共通wgpu描画経路を変更した。

## 起動時にフォルダーが飛ぶ不具合

PR headの汎用`Spring::step`は、最大100msの実フレーム時間を1回で積分していた。
重い起動フレームで半陰的Euler法が不安定になり、フォルダー本体とミニアイコンの
座標が発散する。変更前バイナリでも欠落を確認した。ラベルは別経路なので残る。

`950b994`で、既存のスクローラーと同じ`PhysicsConfig::max_dt`（1/60秒）で
**全経過時間を分割して積分**する。経過時間の切り捨てやアニメーション短縮はしない。
100ms/18fps/35fpsと途中のtarget変更を混ぜた再現テストは、修正前には最初のframeで
座標6108pxへ飛んで失敗し、修正後は収束する。60/120/144fpsでは従来の位置・速度と
完全一致するテストも追加した。Mac・Windows共通修正。

修正済みの比較用・最適化版とも、実ウィンドウの起動5/15/30秒後のself-captureで
フォルダーが残ることを確認した。`target/pr237-stable-startup-{baseline,optimized}`。
最終性能比較にはこの修正を両側に適用し、欠落したsceneを速度比較に使わない。

## 原因の切り分け

- 初期の探索計測（spring修正前）の実ウィンドウで約34fpsを再現。更新処理は約0.1ms、CPUのcommand
  encoding/submissionは約1msだった。時間の多くはsurface取得待ちに現れ、
  `present()` 呼び出し自体は短かった。この待ちにはGPU、driver、compositor、
  FIFO pacingの影響が混在するため、HUDだけから固定上限とは判断していない。
- このMacでは既存GPU timestamp profilerに負値などのinvalid sampleが多く、
  render-pass timestampも信頼できなかった。GPU scopeのp50/p95を根拠にはしていない。
- 別途、submission完了を待つ診断では、下部バーより前のsceneが約42ms、
  bar/上位合成/OS presentationが各約1〜2msだった。この値はqueue/driver待ちを
  含むwall timeであり、純粋なGPU実行時間ではない。同期診断時のFPSも比較対象外。
- BottomBarのキャッシュとpresentationの整理だけでは大きく改善せず、
  folder gridのSDF再生成を止めた静止画面で60fpsになった。編集時の再計算が
  残るため、描画範囲の限定と形状ごとのアニメーション計算の再利用を追加した。

## 実装

1. **Folder gridのgeometry/tintを再利用。** 同じshape/uniformなら既存textureを
   保持する。scroll、時刻、presentation、材質、sizeの変更で更新し、drag、badge、
   settings、context menuなど同じtextureを上書きするlaneは所有権を無効にする。
   背景の画素更新だけではgeometryを無効にしない。
2. **空の画素でのSDF評価を削減。** smooth unionと揺れの最大範囲、
   2×2 derivative quadを含めた、重複しないscissorの和集合を使う。
   Macでは広く連なる範囲も256px単位に分けて形状の距離判定を絞る。
   Windowsは全unionを保持するため分割でshader loopは減らない。`edeef48`で
   merged scissorを維持し、各範囲で同じstorage buffer rangeを共有するようにした。
   無益なdraw増加と形状データの重複転送を避ける。
   出現アニメーション中は元の全画面評価へ戻す。viewport、texture解像度、
   サンプリング座標、unionの順序、陰影の式は変えない。
   Macでは各scissorでSDFの上下限を保守的に評価し、smooth unionが既存入力をそのまま
   選ぶ場合だけ遠い形状を除外する。後の形状が完全に置き換える正距離のprefixも
   除外する。maskとinside-tintを保持し、曖昧な場合は残す。既存storage bufferを
   アラインしたrangeに分け、同じ1つのgeometry passで描く。
3. **Macの編集時sin/cosを形状ごとに毎フレーム計算。** 同じ時刻と同じ式を使い、
   元のGPU計算を全画素で繰り返さない。既存storage bufferのpaddingを使用するので
   strideは96 byteのまま。Metal参照との差は1 RGBA8単位以内。
   Windows CIで7単位の差を検出し、CPU計算を止めても同じ差が残った。
   そのためWindowsでは元のシェーダー三角関数と全形状の逐次合成を維持する。
   `mix(b,a,1)`等も浮動小数点の演算省略で丸め方が変わり得るため、
   数式上の恒等性だけで画素の同一性を判断しない。画素比較の許容値は緩めていない。
   geometryの再利用とscissorによる画素範囲削減は両OSで有効。
4. **BottomBarのgeometryを独立にキャッシュ。** shapeとSDF入力だけをkeyにし、
   fade、動く背景、blur、darknessはmaterialに毎フレーム反映する。歯車とcapsuleは
   既存の1つの完成シーンcheckpointとblurを共有し、追加pass/textureは作らない。
5. **sRGB変換の相殺。** PostMultiplied+sRGBと不透明画素では
   `decode(encode(linear))` を恒等式としてまとめる。必要なattachment側のsRGB変換は
   維持し、WindowsのPreMultipliedでalpha<1のencoded-premultiplicationも維持する。
6. **任意のCSV計測。** CPU更新、surface取得、encode/submit、present呼び出しと
   present呼び出し間隔を記録する。通常実行ではCSV/GPU同期を有効にしない。

背景取得、完成シーンcopy/flatten、blurの解像度・更新頻度、アニメーション速度、
透明度、色、輪郭、レイヤー順は変更していない。低FPS時の位置発散は上記の独立修正。
見た目を落とす設定は採用しない。

## 計測条件と結果

Apple M3 / MacBook Air / 24GB、macOS 15.7.7、Metal、Rust 1.89.0 Release。
実ウィンドウは2560×1602物理pixel、表示倍率2、Fifo/60Hz、Bgra8UnormSrgb、
PostMultiplied。元からあるcapture scaleの既定値（2倍表示では縦横0.5）、
60HzのScreenCaptureKitと動画相当背景を維持した。PNG readbackなし。

`gpu-profile` feature付きReleaseだが、比較時は `LAUNCHPAD_GPU_PROFILE` と
`LAUNCHPAD_PROFILE_SYNC_GPU` を未設定にした。各条件3回、8秒warmup後に再表示、さらに1秒待ち、12秒採取。CSVは起動後10秒を除外。
各条件でbaseline/optimizedを交互に実行し、2巡目は順番を逆にした。
操作なしの実デスクトップは `ANIMATED_BACKDROP=0`、ほかは同じSwift背景fixture。
scrollは既存ハーネスの3秒周期往復、editはproductionの揺れ/削除badge/操作control。
同じユーザーの配置（top-level 41項目、folder 33個）を使用。
常用のLaunchpadを一時停止し、比較中の別ビルド・GPUテストは避けた。
macOSのDBはNSFileManagerのApplication Supportにあり、harnessのHOME指定はDBを
別コピーにする仕組みではない。自動scroll/editは配置変更を行わない。

各runの統計の中央値。フレーム時間はms、σは標準偏差。矢印はbaseline→最適化版。

| 条件 | FPS | 平均ms | p50 ms | p95 ms | p99 ms | σ ms |
|---|---:|---:|---:|---:|---:|---:|
| 操作なし・実デスクトップ | 30.10 → 59.99 | 33.22 → 16.67 | 33.33 → 16.66 | 36.87 → 17.67 | 50.12 → 18.28 | 5.83 → 0.85 |
| 操作なし・動く背景 | 23.51 → 59.99 | 42.53 → 16.67 | 49.68 → 16.66 | 50.43 → 17.92 | 51.02 → 18.53 | 7.01 → 0.88 |
| スクロール・動く背景 | 24.18 → 59.52 | 41.36 → 16.80 | 33.84 → 16.66 | 50.32 → 18.02 | 50.78 → 19.01 | 8.36 → 1.65 |
| 編集・動く背景 | 22.46 → 59.49 | 44.52 → 16.81 | 49.93 → 16.67 | 50.36 → 18.25 | 52.67 → 19.85 | 8.55 → 1.67 |

全runのFPS（採取順。低いrunも除外していない）:

| 条件 | baseline 1 / 2 / 3 | 最適化版 1 / 2 / 3 |
|---|---|---|
| static | 34.93 / 28.74 / 30.10 | 60.00 / 59.70 / 59.99 |
| live | 20.87 / 26.90 / 23.51 | 54.22 / 60.00 / 59.99 |
| scroll | 25.74 / 1.02 / 24.18 | 59.52 / 9.79 / 60.01 |
| edit | 24.82 / 11.44 / 22.46 | 59.49 / 58.81 / 60.01 |

通常の描画は約60Hzまで改善したが、安定した60fpsを全runで達成したとは扱わない。
特にscrollの2巡目には両側で大きなsurface取得待ちが発生した。最適化版の最大frameは
518ms（取得待ち514ms）、baselineは2169ms（取得待ち2162ms）。liveの最適化版1巡目も
単発1016ms（取得待ち1007ms）があり、平均FPSが54.22へ下がった。
同時期にはOSのStorageManagement/Xprotect等の活動も見られたが、原因とは断定しない。
`pmset -g therm`では警告記録なし、memory pressure確認では空き64%だった。
中央値の表だけでこれらの停滞を隠さず、全runのCSVと集計を残した。

編集時のCPU更新平均の中央値は0.128→0.253ms、encode/submitは1.103→2.642ms、
surface取得は43.232→13.792ms、present呼び出しは0.008→0.020ms。
領域別drawの増加でCPU側は少し増えたが、frame間隔は短くなった。
取得待ちにはGPU・driver・compositor・FIFO pacingが混在し、純GPU時間とは呼ばない。

成果物は`target/pr237-final-{baseline,optimized}-{static,live,scroll,edit}-{1,2,3}`、
全runの統計は`target/pr237-evidence/final-performance.json`。
Mac最適化版binaryは`target/pr237-bounded-profile`（`2df3c04`のMac処理。
後続のWindows精度修正ではMac処理は変わらない）、baselineは`950b994`。


以前の探索計測は、フォルダー座標発散の可能性を排除できず、性能改善の最終根拠には
使わない。`target/pr237-before-*` / `pr237-optimized-*` として残しているが、
それらの数字を上の比較へ混ぜていない。

FPSはpresent呼び出し間隔の逆数で、displayに実際に走査されたframe数ではない。
Macの別アプリ/WindowServerの負荷、温度やOS schedulingを完全には固定していない。
QA readback時間や同期GPU診断の数値は、上表へ混ぜていない。
CSVは120frameまたは2秒ごとのバッチ書き込みなので、TERM終了直前の未flush分は含まれない。

## 再現手順

```sh
cargo build --release --locked --features gpu-profile
export DURATION_SECONDS=12 WARMUP_SECONDS=8 FRAME_WARMUP_SECONDS=10
# 各条件3回。各OUTPUT_DIRは未作成の場所にする。
SKIP_BUILD=1 ANIMATED_BACKDROP=0 OUTPUT_DIR=target/static-1 scripts/profile_macos.sh live
SKIP_BUILD=1 OUTPUT_DIR=target/live-1 scripts/profile_macos.sh live
SKIP_BUILD=1 OUTPUT_DIR=target/scroll-1 scripts/profile_macos.sh scroll
SKIP_BUILD=1 OUTPUT_DIR=target/edit-1 scripts/profile_macos.sh edit
python3 scripts/summarize_frame_metrics.py target/live-1/frame-metrics.csv --warmup 10
```

`environment.txt`、`binary-sha256.txt`、`source-status.txt`、`frame-summary.json`、
`frame-metrics.csv`とruntime logを保存する。head SHAだけでは未commitの内容は識別できないため、
実行binaryのSHAとdiffも保管する。`PROFILE_BINARY`で保存済みの比較binaryを指定できる。
今回のbaseline binaryは `target/pr237-stable-baseline-profile` に保存した。
比較用commitは **`950b994`**（`1ea720c` + 計測用`7d1099c` + spring修正）。
別worktree・別target-dirでこのcommitをRelease buildすると、描画最適化なしでCSVを取得できる。
同じtarget-dirを共有するとCargoが別worktreeのartifactを再利用する場合があるため、
別target-dirを使うか`cargo clean --release -p launchpad-windows`してからbuildする。
両binaryのSHA256が異なることも確認する。
現行の `profile_macos.sh` から `PROFILE_BINARY` でそのbinaryを指定して測る。
同commitはCSVの8区間formatを含むが、GPU同期waitはまだ呼び出さない。
同じ計測専用commitのpatchも `target/pr237-evidence/baseline-metrics.patch` に保存した。

診断だけでGPU queue完了待ちを分ける場合:

```sh
LAUNCHPAD_PROFILE_SYNC_GPU=1 OUTPUT_DIR=target/sync-gpu scripts/profile_macos.sh edit
```

`gpu_lower/bar/upper/present_ms` は各区間のqueue完了wall time。
このモードはCPU/GPU overlapを壊すため通常FPSとの比較には使わない。

Windowsでも同じCSV計測と共通最適化が使える。Releaseを作り、同じ画面寸法・DPI・
scene・操作を固定して次を実行し、CSVを同じPython scriptで集計する:

```powershell
$env:LAUNCHPAD_FRAME_METRICS = "$PWD\target\frames.csv"
$env:LAUNCHPAD_DEBUG = '1'
$env:LAUNCHPAD_PROFILE_KEEP_VISIBLE = '1'
$env:LAUNCHPAD_PROFILE_EDIT = '1' # scrollingはLAUNCHPAD_PROFILE_SCROLL
.\target\release\launchpad-windows.exe
```

テスト用の環境変数を通常起動へ持ち越さない。WindowsのGPU timestamp/nvidia-smiと
照合する手順は [PROFILING_EVALUATION.md](PROFILING_EVALUATION.md) を参照。

## 検証と限界

- `cargo fmt --all -- --check` / `git diff --check`: 成功。
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: 成功。
- `cargo test --all-targets --locked`: **1,712 passed / 0 failed / 2 ignored**。
- `cargo build --locked` と `cargo build --release --locked`: 成功。
- production WGSL readback: **65件成功**、bottom bar GPU harness: **26件成功**。
- 実際のRust presentation: RGBA/BGRA、sRGB/Unorm、Pre/PostMultiplied、alpha、
  resizeの128組み合わせを含む2テストが成功。
- 追加したgrid参照テスト: 再利用、共有writer後の再描画、scrollで画面外へ出て戻る形状、
  blend 0/2/16/32/64、30個の異なるtint、複数行/列、時刻、presentation途中、resizeを確認。
  full redrawと最適化のRGBA8最大差は1以下（CPU/GPU三角関数の丸め差）。
  同一入力の再利用と共有writer後の復帰は完全一致。
- bar参照テスト: capsule/gearの背景共有、前景の保持、fade/AA、背景差し替え、
  capture-loss/recovery、resize、形状/材質変更、smooth-union bridgeを確認。
- 連番QAは7条件**1,251フレーム**。settings toggle/slider、folder interactions、
  edit/drag/wiggle、倍率1/1.5/2（1280×800 / 1920×1200 / 2560×1600）。
  汎用artifact verifierはすべて成功。歯車のガラス輪郭、完了ボタンとの分離、
  フォルダーと削除badge、文字をPNGで確認。

### 自動QAの既存制約

連番QAはframe時刻を固定するが、control等の一部は実時間にも依存する。
manifestの `bottom_glass_rects` にも実行ごとの差があるため、全PNGのbit一致は
成立しなかった。RGBA全画素の比較を行い、変更前同士の再実行でも差が出ることを確認した。
最新のspring修正済みbaselineとの平均RGBA差は、edit/drag 0.0056、folder 0.1382、
slider 0.1835、toggle 0.3655（0〜255単位）。局所的な最大差は大きく、
平均だけで同一画質を証明したとは扱わない。探索中のbaseline同士の再実行でも、
toggle 0.679等の差が出た。
これは「全フレームで見た目が完全一致」の証明ではない。同じ入力を固定した
production rendererの参照テストを正確性の主な根拠とした。
最新の比較は`target/pr237-evidence/rgba-stable-bounded.json`、
画像は`target/pr237-stable-baseline-qa/qa-sequences`と
`target/pr237-bounded-qa/qa-sequences`に保存した。

専用verifierの `search ignores darkness`（toggle）と
`unbounded or missing horizontal puff`（slider）は**変更前後とも同じ失敗**。
これらを成功とは数えていない。標準Debugテストは全成功だが、追加で試した
`cargo test --release --all-targets` ではDebug専用設定UIを前提とする既存11テストが
失敗した（今回それらのコードは変更していない）。

### Windows CI

`5f1bc00`の[Windows checks](https://github.com/kamakiri1192/windows-launchpad-rust/actions/runs/42402953027)
でfmt、全target/all-feature Clippy、tests、buildが成功した。新しいgrid参照比較と
capsule/gear lifecycle比較も実GPU経路で成功。
[Liquid Glass GPU regressions](https://github.com/kamakiri1192/windows-launchpad-rust/actions/runs/42402953041)
のproduction WGSL readbackも成功した。

### 未検証

- Windows実機のDX12/DirectComposition、FPS、native capture、HDR/色profile。
  Windows-GNU cross-checkは `x86_64-w64-mingw32-gcc` 不在によるbundled SQLiteの
  compile失敗で停止したが、上記のnative Windows CIでは成功した。
  Windows実機の対話操作・FPS確認は未実施。
- OSウィンドウの実操作resize、別DPIモニター間の移動。
  GPU resourceのresizeと異なる倍率のQAは確認したが、OSのイベント経路は未確認。
- 上記のQA時計差を含む、全アニメーションframeの完全なpixel一致。
- 動く背景は通常経路のcaptureを止めずに実測した。SCKの毎frame取り込みと
  背景差し替えGPUテストを確認したが、全種類の動画・外部アプリ背景は網羅していない。


## 追加依頼: 編集時のアプリ名も一緒に揺らす

`59d86a3`で、トップ画面のアプリ/フォルダー名とフォルダー内のアプリ名を、
影ごとアイコンと同じpivot・位相・回転・上下動・scale/liftへ追従させた。
閉じたフォルダーはミニアイコン群のrigid motionに合わせる。通常の設定文字、
フォルダーの見出しは動かさない。glyph/atlasは時刻だけでは作り直さず、
既存のGPU uniform clockで動かす。ownerの位置/flag変更時には文字bufferも更新する。

自己レビューでフォルダー端のdrag中に文字がscissorで切れるケースを見つけ、
ドラッグ文字を同じownerの前景drawへ移した。安定したowner IDで対応づけるので、
並べ替えでdraw順が変わっても別アイコンの位相を使わない。
文字/影を同じbuffer内で安定分割して既存drag passへ描くため、描画passは増やさない。

`qa/edit_label_wiggle.json` と `scripts/verify_edit_label_wiggle.py` を追加。
Latin/Japanese/折返し、トップ3件/フォルダー内3件、左右の揺れ、編集終了、
フォルダー端を越えるドラッグをPNG画素から確認する。各frameのGPU時計とownerの
位置・位相を記録し、静止画の文字をアイコンと同じ剛体変換で予測した結果と比較する。
文字が静止したままの変更前PNGを代入すると検証が失敗することも確認した。

最終の7条件は**2,120 frames**（倍率1/1.5/2の専用連番、既存edit/drag、folder
interaction、folder page drag、label-click exit）。汎用artifact verifierは全成功。
専用画素検証も3倍率すべて成功。編集終了後はidle画像と一致、端を越えるdragの
最低画像類似度は0.979 / 0.990 / 0.993（合格閾値0.85）。この指標は本fixtureの
白い文字maskの類似度で、任意の画面全体の同一性を表すものではない。

画像: `target/pr237-label-final-qa/qa-sequences`。
動画: `target/pr237-evidence/edit-label-wiggle.mp4`。
手順: [EDIT_MODE_VISUAL_QA.md](EDIT_MODE_VISUAL_QA.md#automated-label-motion-regression)。
通常の設定toggle/sliderとfolder top-level dragも追加前後の連番で確認した。

最終の標準Macチェックは `cargo test --all-targets --locked` **1,713 passed /
0 failed / 2 ignored**、all-target/all-feature Clippy（warnings denied）、fmt、
Release buildが成功。

### 文字の揺れ追加後の性能再確認

同じMac/画面/背景/配置/Releaseと8秒warmup・12秒測定で、追加前後を交互に
3回ずつ比較した（2巡目は逆順）。今回は両側とも `caffeinate -di` で無操作時の
sleepを抑止した。計測中のビルド・GPU QA・PNG readbackは行っていない。
追加前は `target/pr237-bounded-profile`、追加後は `59d86a3` の
`target/pr237-label-profile`。`edeef48`の変更はWindows専用の範囲削減と
同一形状range再利用なので、このMac計測の通常描画には影響しない。

| 編集・動く背景（各runの中央値） | 文字追加前 | 文字追加後 |
|---|---:|---:|
| FPS | 60.00 | 60.00 |
| 平均 / p50 ms | 16.667 / 16.699 | 16.665 / 16.689 |
| p95 / p99 ms | 18.188 / 18.909 | 18.325 / 18.793 |
| 標準偏差 ms | 0.998 | 1.089 |

全3回のFPSは追加前59.999 / 59.989 / 60.000、追加後60.004 / 60.013 /
60.002。静止時も1回ずつ追試し、60.017→59.998fps、平均16.662→16.667ms、
p95 17.639→17.717ms、σ 0.663→0.770msだった。
文字の揺れを加えた条件でも通常の60Hz描画を維持できた。
この追試は先の全4条件で観測した長いsurface取得待ちが解消した証明ではない。
生CSVは `target/pr237-label-perf-{before,after}-{edit,live}-*`、
集計は `target/pr237-evidence/label-performance.json`。

追加後の計測binary SHA256:
`b5612cd6b2e4c912d5286377b50a854f77c515af94de8523f07aaa6eb4bbbc09`。

### 最終コードのWindows確認

`edeef48`の[Windows checks](https://github.com/kamakiri1192/windows-launchpad-rust/actions/runs/42404966098)
でfmt / all-target・all-feature Clippy / 全テスト / buildが成功。
[Liquid Glass GPU regressions](https://github.com/kamakiri1192/windows-launchpad-rust/actions/runs/42404966140)
も成功。文字shaderの96-byte instance layoutを含む実GPU pipeline検証と、
grid/full-redraw参照、bar/gear lifecycle参照を通過した。
[Windows binary](https://github.com/kamakiri1192/windows-launchpad-rust/actions/runs/42404966146)と
[Mac application](https://github.com/kamakiri1192/windows-launchpad-rust/actions/runs/42404966143)
のartifact buildも成功。既存条件によりMac CI checksとnative-input E2Eはskipされるため、
Macの検証根拠は上記の実機テスト・連番・計測とする。

自己レビューではcache key/invalidation、共有textureの全writer、色変換の境界、
GPU buffer stride、文字と影のowner追従、drag時のclipとdraw順、Windowsの無益な
細分化を点検した。見つけたdrag文字の欠けとWindowsの重複処理は修正済み。
常用の `Launchpad 5.app` は計測後に再起動した。既存bundleは上書きしていない。
