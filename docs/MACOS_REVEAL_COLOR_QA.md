# macOS 出現時の色・背景検証

GPU self-capture / headless QA は **WindowServerによる合成前** の画像である。
最終的なMacウィンドウの透明色の解釈や、非表示前のdrawableの再表示は
それだけでは検証できない。この検証では実ウィンドウ、実背景取得、OS録画を使う。

## 原因と修正範囲

1. wgpu 29.0.3のMetalは `PostMultiplied` を広告するが、透明なCAMetalLayerは
   premultipliedの保存色を合成する。
   [上流の修正 #9922](https://github.com/gfx-rs/wgpu/pull/9922) とローカルHAL実装を確認した。
   従来の最終変換はstraight RGBを渡していたため、フェード途中で背景への加算が
   過大になり、青い背景が明るい水色に跳ねた。surfaceの設定は対応している
   PostMultipliedのまま、**Metalだけ** 最終保存色をPreMultipliedとして変換する。
   DX12等の本来のPre/Post契約、sRGB変換、ガラス内部のlinear合成は維持する。
2. Metalの `acquire_texture` は非表示のNSWindowに `Occluded` を返す。
   `summon` 内の非表示中の描画では古いdrawableを更新できず、再表示直後に
   以前の不透明な画面が先に見えた。非表示時にCAMetalLayerのopacityを0にし、
   再表示後の最初の実drawableとopacity=1を同じCore Animation transactionで
   公開する。非表示直後に残ったredrawでは描画しない。再開前の未消費captureを
   破棄し、実drawableと新しい背景が揃うまでは出現の時計を0で保持する。
   capture失敗・無効時には既存のfallbackへ進む。
   transactional presentは最初の公開までに限定する。通常のFIFO描画、アニメーションの
   時間・軌道、背景取得頻度、blur、解像度、bar/gearの共有passは変えない。

## 再現手順

他のLaunchpadを終了し、対象アプリと録画実行元に画面収録の権限、入力生成元に
アクセシビリティの権限を与える。既存プロセスを検出した場合ハーネスは停止する。
操作中はキーボード・マウスを触らない。全ディスプレイに既知の背景を表示し、
録画するディスプレイだけを `--display` で選ぶ（標準は1）。

```sh
bash scripts/build_macos_app.sh
python3 scripts/qa_macos_reveal.py --app dist/Launchpad.app \
  --output target/reveal-static
# 上のハーネスが通常起動し直したLaunchpadを終了してから次を実行する
python3 scripts/qa_macos_reveal.py --app dist/Launchpad.app \
  --output target/reveal-dynamic --dynamic
# 同様に終了してから、取得不可時にも開けることを確認する
python3 scripts/qa_macos_reveal.py --app dist/Launchpad.app \
  --output target/reveal-fallback --fallback
```

`native.mov` はOS合成後、`settled.png` はGPU合成前。`app.log` はキャプチャの
方式・状態、`debug.log` は実際のhide/summon、`events.json` は操作時刻を記録する。
ハーネスは既定で3回の実際の非表示→表示を検証し、取得できていない場合は失敗する。
終了時に補助プロセスを閉じ、対象のアプリを通常起動する。実際のユーザー配置を
表示するが、並べ替えや設定変更はしない。生成物は `target/` 内に保管する。

```sh
ffmpeg -i target/reveal-dynamic/native.mov -vsync 0 \
  target/reveal-dynamic/frame-%04d.png
cargo test --test surface_presentation -- --nocapture
```

確認点:

- 出現の開始から完了まで、白やシアンへの跳ね・反転がない。
- 動く背景を非表示中も進め、再表示時に前回の不透明な画面が先に出ない。
- 背景の色と模様がガラス内でも追従し、静止後も更新が続く。
- アイコン・文字の動き、ガラスの輪郭・ぼかし、下部バー・歯車を維持する。
- debugログで `visible was false` が3回、captureログで実GPU取得を確認する。

GPUテストは4 texture形式、4 backend/alpha組合せ、2サイズ、2色、4透明度の
256条件を実際のRust presentation pipelineで確認する。Metalの旧Post表記は
premultiplied、将来のPre表記もpremultiplied、DX12 PreとVulkan Postは既存の
契約のまま。これはnative compositor検証の代わりではない。

## 2026-11-08の実測

Apple M3 / macOS 15.7.7、Release、Metal/Fifo 60Hz、アプリ2560×1602、倍率2。
比較用OS録画は3558×2304、60Hz・可変frame間隔。元からあるcapture scale
（倍率2では0.5）は変更していない。初期切り分け用の3420×2214録画は数値比較から除外した。

修正前 `8e0cb78` とalpha修正後を同じ静止背景・3回の非表示/再表示で比較した。
固定ROI `(1000,700,20,120)` の平均緑値が180/255を超える
シアンの跳ねは修正前682枚中21枚、修正後681枚中0枚（重複なしでdecodeした画像）。ROIの最大RGBは
`106.04,194.55,229.89` → `61.96,129.00,203.27`。
これは特定の背景・ROIでの再現指標であり、一般的な画質評価指標ではない。
実画面録画でもフェード中の跳ねを確認し、GPUの静止画像だけでは見えないことを確認した。

初回の追加検証で、alpha修正とは別に古いdrawableの再表示も発見した。
このため静止背景だけでなく、上記の動的背景によるnative検証を必須とする。

最終版の動的背景で3回の出現を全コマ確認し、以前の不透明な画面が先に出る
現象が消えた。色と模様は再開したcaptureに追従した。
静止背景の最終追試（decodeした739枚）でも同じROIのシアン跳ねは0フレーム、
最大RGBは `61.96,129.07,203.17` だった。
録画とGPU self-captureは `target/reveal-color-qa/final-{static,dynamic,fallback}` に保存。
動画コンテナ上のsample数は静止742、動的背景726、取得無効のfallback743。各3回の再表示を確認し、
fallbackも表示待ちで止まらなかった。PNG展開は `-vsync 0` で複製を入れずに行う。

Macの `cargo test --all-targets` は **1,719 passed / 0 failed / 2 ignored**。
fmt、全target・全feature Clippy（warnings denied）、Debug/Release buildも成功。
自己レビューでは非表示直後のqueued redraw、初回drawableの公開、capture再開、
失敗通知の保持、capture無効/空形状時の待ち解除、非Metalのalpha契約を確認した。

録画時は意図的に非表示の時間を含むため、ログの区間平均FPSは速度比較に使わない。
HDR、別GPU/別macOS、Windows実機のOS合成はこのMac録画の検証範囲外。
