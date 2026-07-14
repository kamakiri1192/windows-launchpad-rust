# GPU / CPU パフォーマンス計測ガイド

## 目的

この文書は、Launchpad の「カクつく」「GPU 使用率が高い」といった問題を、見た目だけで推測せずに計測するための手順書です。主に次の4点を切り分けます。

1. プロセス全体の CPU 使用率
2. プロセス全体の GPU 使用率
3. wgpu の描画スコープごとの GPU 実行時間
4. 入力、スクロール、snap、再レイアウトの状態遷移

計測のために backdrop 更新、メイン画面の再レイアウト、文字組みなど、通常動いている処理を先に止めてはいけません。最初は production と同じ構成を測り、原因を数値で絞った後に、変更を1つずつ比較します。

## 計測方法の使い分け

| 方法 | 分かること | 分からないこと | 主な用途 |
|---|---|---|---|
| PowerShell `Get-Process` | アプリ全体の平均 CPU 使用率、ワーキングセット | CPU 関数別の時間、GPU の内訳 | CPU bound かどうかの初期判定 |
| `nvidia-smi pmon` | NVIDIA GPU 上のプロセス単位の SM、メモリエンジン、VRAM 使用量 | wgpu のどの描画パスが重いか | GPU bound かどうかの初期判定 |
| `wgpu-profiler` | Liquid Glass、blur、modal content などの GPU 時間 | CPU の待ち時間や入力競合 | GPU ボトルネックの特定 |
| QA telemetry | pointer、scroll、snap、relayout の時系列 | 通常表示時の絶対 GPU 性能 | ロジック競合や状態遷移の確認 |

基本の順番は次の通りです。

1. release build で問題を再現する。
2. `Get-Process` と `nvidia-smi pmon` で CPU/GPU のどちらが支配的かを見る。
3. GPU 側が疑わしければ `wgpu-profiler` で描画パス別時間を測る。
4. 使用率は低いのに動きが不自然なら、QA telemetry で入力と physics の競合を調べる。
5. 同じ条件で変更前後を比較する。

## 共通の計測条件

比較可能な結果にするため、最低限、次を記録します。

- commit hash
- Debug/Release と Cargo feature
- GPU 名、ドライバ、wgpu backend
- viewport または実画面の解像度と Windows の拡大率
- モニターのリフレッシュレート
- 通常表示、トップレベル編集、フォルダ表示、フォルダ編集などの状態
- 操作内容と計測時間
- backdrop の有無
- QA シナリオの場合はシナリオ名、fps、PNG readback の有無

計測は release build で行い、起動直後の shader compilation やリソース読み込みが落ち着いてから開始します。単一の瞬間値ではなく、同じ操作を少なくとも3回、使用率は5サンプル以上取ります。変更前後では解像度、状態、操作時間、fps 上限を揃えます。

## 1. PowerShell でプロセス全体の CPU 使用率を測る

### 実行方法

別の PowerShell で release build のアプリを起動し、問題の状態を作ります。

~~~powershell
cargo build --release
.\target\release\launchpad-windows.exe
~~~

計測用の PowerShell で次を実行します。この例は2秒間の平均です。

~~~powershell
$process = Get-Process -Name launchpad-windows -ErrorAction Stop |
    Select-Object -First 1
$logicalCpuCount = [Environment]::ProcessorCount
$cpuStart = $process.TotalProcessorTime
$timer = [Diagnostics.Stopwatch]::StartNew()

Start-Sleep -Seconds 2

$process.Refresh()
$timer.Stop()
$cpuPercent = 100 *
    ($process.TotalProcessorTime - $cpuStart).TotalSeconds /
    $timer.Elapsed.TotalSeconds /
    $logicalCpuCount

[pscustomobject]@{
    Pid           = $process.Id
    WindowSeconds = [math]::Round($timer.Elapsed.TotalSeconds, 2)
    CpuPercent    = [math]::Round($cpuPercent, 2)
    WorkingSetMB  = [math]::Round($process.WorkingSet64 / 1MB, 1)
} | Format-Table -AutoSize
~~~

### 計算内容

`TotalProcessorTime` は、対象プロセスが全スレッドで消費した CPU 時間の累積値です。

~~~text
CPU使用率 = CPU時間の増分 ÷ 実時間 ÷ 論理CPU数 × 100
~~~

この文書ではマシン全体を100%とする値に正規化します。例えば16論理CPUのうち1論理CPUを完全に使った場合は約6.25%です。短いスパイクは2秒平均では薄まるため、フレーム単位の CPU 内訳が必要なら、後述の Puffin または Tracy を検討します。

## 2. `nvidia-smi pmon` でプロセス全体の GPU 使用率を測る

### 事前確認

~~~powershell
nvidia-smi
nvidia-smi pmon -h
~~~

`pmon` は NVIDIA GPU のプロセス単位カウンタです。ドライバ、GPU、Windows の WDDM 状態によって取得できない列があり、その場合は `-` と表示されます。`-` は0%ではなく「取得不能」です。

### 5秒間の計測

アプリを問題の状態にした後、別の PowerShell で実行します。

~~~powershell
$appPid = (Get-Process -Name launchpad-windows -ErrorAction Stop |
    Select-Object -First 1).Id

nvidia-smi pmon -s um -d 1 -c 5 |
    Select-String -Pattern "^\s*\d+\s+$appPid\s"
~~~

- `-s u`: SM、memory engine、encoder/decoder などの使用率
- `-s m`: framebuffer memory などの使用量
- `-s um`: 上記を同時に取得
- `-d 1`: 1秒間隔
- `-c 5`: 5サンプルで終了

主に見る列:

| 列 | 意味 |
|---|---|
| `sm` | 前回サンプルから今回までに GPU の演算器が動いていた割合 |
| `mem` | GPU のデバイスメモリが読み書きされていた割合。VRAM 容量の占有率ではない |
| `fb` | 使用中の framebuffer memory。通常は MB |
| `enc` / `dec` | 動画エンコード、デコードエンジンの使用率 |

`sm` が継続的に高く、CPU 使用率が低ければ、まず GPU bound を疑います。ただし `sm` だけでは blur、Liquid Glass、テキストなどの内訳は分かりません。次の `wgpu-profiler` を使います。

公式仕様: [NVIDIA System Management Interface — Process Monitoring](https://docs.nvidia.com/deploy/nvidia-smi/index.html#process-monitoring)

## 3. `wgpu-profiler` で描画パス別の GPU 時間を測る

### このリポジトリでの構成

`wgpu-profiler` は optional Cargo feature `gpu-profile` として導入しています。

- 通常の `cargo build --release` では依存関係自体をコンパイルしません。
- `--features gpu-profile` 付きでも、`LAUNCHPAD_GPU_PROFILE` がなければ timestamp-query の GPU feature を要求せず、query resource も作りません。
- feature と環境変数の両方があり、adapter が `TIMESTAMP_QUERY` を持つ場合だけ計測します。
- GPU 結果は非同期で回収し、結果待ちのための意図的な frame stall は入れません。

参照:

- [wgpu-profiler 0.27](https://docs.rs/crate/wgpu-profiler/0.27.0)
- [wgpu timestamp query example](https://wgpu.rs/doc/wgpu_examples/timestamp_queries/index.html)

### 計測しているスコープ

- `lower_scene_clear`
- `base_liquid_glass`
- `grid_tile_fill`
- `folder_grid_glass`
- `grid_icons_text`
- `edit_badge_glass` / `edit_badge_ink`
- `top_level_drag_overlay`
- `drag_folder_liquid_glass`
- `control_liquid_glass` / `control_content`
- `focus_blur_downsample_1` 〜 `focus_blur_downsample_3`
- `focus_blur_upsample_1` 〜 `focus_blur_upsample_3`
- `focus_blur_composite`
- `focus_veil_tint`
- `modal_liquid_glass`
- `modal_content`

### 手動操作を計測する

~~~powershell
cargo build --release --features gpu-profile

$target = (Resolve-Path .\target).Path
$env:LAUNCHPAD_GPU_PROFILE = Join-Path $target 'gpu-profile-manual.json'
.\target\release\launchpad-windows.exe
~~~

標準エラーに次が出れば有効です。

~~~text
gpu profiler enabled: ... report=...\gpu-profile-manual.json
~~~

adapter が対応していない場合や初期化に失敗した場合は、`gpu profiler disabled: ...` と表示されます。レポートは最初の完了フレームと、その後30完了フレームごとに更新されます。終了直前の最大29フレームがファイルへ反映されない場合があるため、計測状態を少なくとも数秒維持します。

### 決定的 QA シナリオを計測する

同一の viewport、fixture、操作で再測定するときに使用します。

~~~powershell
cargo build --release --features gpu-profile

$target = (Resolve-Path .\target).Path
$env:LAUNCHPAD_GPU_PROFILE = Join-Path $target 'folder-scroll-gpu-profile.json'
$env:LAUNCHPAD_QA_SCENARIO =
    (Resolve-Path .\qa\folder_single_page_scroll.json).Path

.\target\release\launchpad-windows.exe
~~~

シナリオの `duration_ms` に達すると自動終了します。ただし連番 QA は各フレームで PNG readback を行うため、通常表示の絶対性能とは条件が異なります。これは状態遷移を固定した比較と profiler のスモーク確認に使い、4K の手動操作と数値を直接比較しません。

計測後は、次の通常実行に環境変数を持ち越さないように削除します。

~~~powershell
Remove-Item Env:LAUNCHPAD_GPU_PROFILE -ErrorAction SilentlyContinue
Remove-Item Env:LAUNCHPAD_QA_SCENARIO -ErrorAction SilentlyContinue
~~~

### 出力

`LAUNCHPAD_GPU_PROFILE` に指定した場所へ2ファイルを出力します。

~~~text
target/
├── folder-scroll-gpu-profile.json
└── folder-scroll-gpu-profile.trace.json
~~~

集計 JSON:

- `finished_frames`: 非同期回収まで完了したフレーム数
- `window_samples_per_scope`: scope ごとの保持上限。現在は240
- `samples`: その scope の集計に使ったサンプル数
- `p50_ms`: 通常時に近い中央値
- `p95_ms`: 遅い側5%付近。継続的な引っかかりの比較に使う
- `max_ms`: 最大値。単発異常の手掛かりだが、1回だけで原因と断定しない

Chrome Trace JSON は Chrome/Chromium の trace viewer で開き、最新の回収済みフレームにおける scope の親子関係と時間軸を確認します。集計値で遅い scope を見つけ、trace で同じフレーム内の並びを確認します。

### 結果の読み方

1. `samples` が十分あることを確認する。
2. 最初に `p95_ms` を比較し、恒常的に重い scope を探す。
3. `max_ms` だけ突出する場合は、trace と CPU/入力 telemetry で単発要因を確認する。
4. blur は downsample、upsample、composite の合計としても見る。
5. Liquid Glass は base、folder grid、control、modal を個別と合計の両方で見る。
6. viewport のピクセル数が違う計測を、そのまま比較しない。
7. 変更後は同じシナリオを最低3回実行し、p50/p95 の傾向を見る。

## 4. QA telemetry でスクロール競合を調べる

GPU 使用率が低くても、入力と snap が同じ位置を交互に書き戻すと、見た目はカクつきます。`LAUNCHPAD_QA_SCENARIO` の `manifest.json` には、次の値がフレームごとに入ります。

- 実フレーム時間
- pointer 座標と move 回数
- scroll 位置、速度、physics phase
- pointer から算出した期待 scroll 位置との誤差
- snap target
- 再レイアウト回数
- 子アプリまたはトップレベルの drag 所有状態

連番 QA の詳しい使い方は [GPU シナリオ連番 QA](GPU_SEQUENCE_QA.md) を参照してください。連番 PNG と telemetry はロジック確認用であり、PNG readback を含むため絶対的な GPU ベンチマークには使いません。

## 標準的な調査手順

例として「フォルダ編集時だけスクロールがカクつく」を調べる場合:

1. 通常表示、トップレベル編集、フォルダ表示、フォルダ編集の4状態を同じ解像度で用意する。
2. 各状態で PowerShell の CPU 平均を3回取る。
3. 各状態で `nvidia-smi pmon` を5〜10サンプル取る。
4. CPU が低く `sm` が高ければ、`wgpu-profiler` を有効にして同じ4状態を測る。
5. `p95_ms` が高い scope を特定する。
6. 使用率が低いのに動きだけ不自然なら、`folder_single_page_scroll.json` と `manifest.json` で入力、Dragging、Settling、Idle の順序を確認する。
7. 原因候補を1つだけ変更し、同じ条件で再測定する。

次のような結論は避けます。

- CPU 使用率が低いという理由だけで、再レイアウトが不要と判断する。
- GPU 使用率が高いという理由だけで、backdrop 更新や blur を止める。
- QA readback ありの数値と、4K の通常表示を直接比較する。
- 最大値1回だけを根拠に最適化する。
- 同時に複数の処理を止めて、何が効いたか分からなくする。

## 2026-07-15 の基準値

計測環境: NVIDIA GeForce RTX 3080 / DX12。

外形監視:

- 手動確認中の常駐 `launchpad-windows` は、`nvidia-smi pmon -s um` の5サンプル連続で SM `98〜99%`、memory engine `2〜3%`。
- 同時に PowerShell の2秒平均で、マシン全体を100%とした CPU 使用率は約 `3.2%`。
- この時点で「CPU 全体より GPU 実行が支配的」と判断できたが、GPU 内のどのパスかは未特定だった。
- 決定的 QA のトップレベル編集は、観測できたサンプルで SM `4〜8%`。フォルダ表示、編集、スクロールを含むシナリオは `2〜12%`。
- QA の scroll telemetry では、1ページフォルダの直接操作中の期待 scroll 位置と実位置の最大誤差は `0px`。同じフレームで scroll と snap が位置を書き戻す競合は観測されなかった。

`wgpu-profiler` のスモーク計測:

1280×800 の `qa/folder_single_page_scroll.json` を feature 付き release で実行し、180完了フレームを timestamp query で計測しました。

| GPU scope | p50 ms | p95 ms | max ms |
|---|---:|---:|---:|
| modal content | 0.0892 | 0.2353 | 1.0753 |
| focus blur composite | 0.0174 | 0.1659 | 0.1740 |
| base Liquid Glass | 0.0575 | 0.1597 | 0.1628 |
| focus veil tint | 0.0134 | 0.1106 | 0.1218 |
| modal Liquid Glass | 0.0429 | 0.1054 | 0.1094 |
| control Liquid Glass | 0.0420 | 0.1044 | 0.1066 |

このスモーク計測では、単独の blur pass は突出していません。1280×800、非表示ウィンドウ、連番 PNG readback という条件なので、4K 手動操作時の `98〜99%` の原因をこの結果だけで断定しません。

## 補助ツールの候補

### Puffin

CPU の `tick_frame`、`relayout`、text shaping、`Renderer::prepare`、command encoding など、Rust の関数や scope 単位を軽量に測る候補です。`wgpu-profiler` にも Puffin 連携があり、GPU 結果との照合に利用できます。現在このリポジトリには未導入です。

- [Puffin](https://docs.rs/puffin/latest/puffin/)

### Tracy

CPU/GPU、複数スレッド、コンテキストスイッチまで追う長期的な候補です。viewer の準備とバージョン整合が必要で、設定によっては profiler discovery をネットワークへ公開します。最初の描画パス別 GPU 時間の特定には、現在導入済みの `wgpu-profiler` の方が小さく使えます。

- [Tracy](https://github.com/wolfpld/tracy)
- [tracy-client](https://docs.rs/tracy-client/latest/tracy_client/)

## 計測記録テンプレート

PR または issue へ次の形式で残します。

~~~text
commit:
build: release / features:
GPU / driver / backend:
viewport / scale / refresh rate:
state:
operation:
duration / samples:

CPU:
  average:
  working set:

nvidia-smi pmon:
  sm:
  mem:
  fb:

wgpu-profiler:
  finished_frames:
  top p95 scopes:
  max outlier:
  report path:
  trace path:

QA telemetry:
  scenario:
  scroll error:
  phase transition:
  relayout count:

conclusion:
next experiment:
~~~
