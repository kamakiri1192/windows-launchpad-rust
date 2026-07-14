# GPU / フレームプロファイリング導入検討

## 目的

フォルダ内スクロールの体感上の引っかかりと、編集モード中の高い GPU 使用率を、入力・CPU レイアウト・GPU 描画パスに分離して計測する。計測前に backdrop 更新や既存レイアウトを止めることはしない。

## 2026-07-15 時点の観測

計測環境は NVIDIA GeForce RTX 3080 / DX12。`nvidia-smi pmon -s um` を1秒間隔で使用した。

- 手動確認中の常駐 `launchpad-windows` は5サンプル連続で SM 使用率 `98〜99%`、メモリ処理 `2〜3%` だった。
- 決定的 QA のトップレベル編集は観測できたサンプルで SM `4〜8%`、フォルダ表示・編集・スクロールを含むシナリオは `2〜12%` だった。
- QA は連番 PNG の GPU readback を含み、非表示ウィンドウを固定時刻で駆動する。したがって絶対値の性能比較には使わず、状態遷移と相対的な傾向だけに使う。
- 既存の scroll telemetry では、1ページフォルダの直接操作中に「pointerから算出した期待scroll位置」と実位置の誤差は最大 `0px` だった。scroll と snap が同じフレームで位置を書き戻す競合は観測されていない。

## 特定できた高 GPU 使用率の原因

編集モードは wiggle を継続するため、各フレーム末尾で次の redraw を要求する。これ自体は必要な動作だが、Surface が `Mailbox` を優先していたため、表示されない中間フレームも可能な限り生成していた。

wgpu の `Mailbox` は新しいフレームで待ち行列を置き換える方式で、DX12 では `desired_maximum_frame_latency * monitor Hz` まで動作し得る。一方 `Fifo` は VBlank ごとの表示待ち行列が空くまで `get_current_texture()` が待機する。ランチャーは低遅延ゲームではなく連続 UI アニメーションなので、`Fifo` を優先する方が適切である。

参照:

- [wgpu PresentMode](https://wgpu.rs/doc/wgpu/enum.PresentMode.html)
- [wgpu SurfaceConfiguration](https://wgpu.rs/doc/wgpu_types/struct.SurfaceConfiguration.html)

今回の修正では `Fifo` を優先する。backdrop の取得・更新条件、メイン画面の再レイアウト、wiggle の更新は停止しない。

## フォルダ表示中に追加される GPU パス

コード上、開いたフォルダでは通常の下層シーンに加えて、次のフルスクリーンまたは広域パスが毎フレーム動く。

1. Grid Overlay Liquid Glass geometry / final
2. Control Liquid Glass geometry / final
3. Dual-Kawase focus blur の downsample / upsample / composite
4. Modal Liquid Glass geometry / final
5. フォルダ内×バッジ用 Liquid Glass geometry / final

NVIDIA のプロセス全体カウンタだけでは、この内訳ごとの GPU 時間は確定できない。特に focus blur が主要因か、複数 Liquid Glass パスの合計が主要因かは timestamp query で分離する必要がある。

## 導入候補

### 第一候補: `wgpu-profiler` 0.27

このリポジトリの `wgpu = 29` と同じ wgpu 29 を使用する版で、GPU timestamp query、ネストしたスコープ、非同期の結果回収、Chrome Trace JSON 出力を備える。計測結果を待つためにフレームを stall しない点も今回の用途に合う。

- [wgpu-profiler 0.27](https://docs.rs/crate/wgpu-profiler/0.27.0)
- [wgpu timestamp query example](https://wgpu.rs/doc/wgpu_examples/timestamp_queries/index.html)

別ブランチで optional feature `gpu-profile` として導入し、通常ビルドには依存・timestamp feature・readback を持ち込まない。adapter が `TIMESTAMP_QUERY` を持つ場合だけ有効化する。

最初に次のスコープを置く。

- base Liquid Glass
- grid overlay Liquid Glass
- grid tile / icon / text
- edit badge
- control Liquid Glass / ink
- focus blur downsample / upsample / composite
- modal Liquid Glass
- modal content / modal badge

出力は各パスの p50 / p95 / max GPU ms と Chrome Trace JSON とし、「通常表示」「トップレベル編集」「フォルダ編集＋スクロール」の3シナリオを同じ解像度で比較する。

### CPU 側: Puffin

CPU の `tick_frame`、`relayout`、text shaping、`Renderer::prepare`、command encoding を見る用途には Puffin が軽量で、スコープは実行時に無効化できる。`wgpu-profiler` 自体にも Puffin 連携があるため、GPU 結果と同じフレームで照合できる。

- [Puffin](https://docs.rs/puffin/latest/puffin/)

### Tracy

CPU/GPU、スレッド、コンテキストスイッチまで追う長期的な選択肢。ただし viewer の準備とバージョン整合が必要で、設定によってはネットワーク上へ profiler discovery を公開する。最初のパス別 GPU 時間の特定には `wgpu-profiler` の方が小さく導入できる。

- [Tracy](https://github.com/wolfpld/tracy)
- [tracy-client](https://docs.rs/tracy-client/latest/tracy_client/)

## 次ブランチの完了条件

- QA readback を無効にした再現ランナーで計測する。
- 同一 viewport / 同一fps上限 / 同一fixtureで3状態を比較する。
- CPU frame time と GPU pass timeを同一フレーム番号で保存する。
- 最も遅いGPUパスを数値で特定してから、解像度・更新頻度・パス統合などの対策を選ぶ。
- backdrop 更新やメイン再レイアウトを止める案は、計測結果が直接それを示すまで採用しない。
