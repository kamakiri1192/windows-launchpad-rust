# 設定パネルのフレーム性能調査と retained UI 方針

## 目的

設定パネル表示中、特に Debug カテゴリをスクロールするときのフレーム落ちを減らす。
ただし、このアプリでは見た目が機能要件なので、背景や Liquid Glass の結果を静止画として扱う最適化は行わない。

## 守る描画契約

- デスクトップキャプチャを含む背景は、常に 60 fps で変化し得るものとして扱う。
- 設定パネル用の completed scene capture、flatten、全面 blur、glass composite は毎フレーム実行する。
- blur 結果のフレーム間キャッシュ、低解像度化、フレームスキップは行わない。
- パネルの AABB を理由に設定 UI の glass surface を事前に捨てない。全 surface を renderer に渡し、ScrollView 自身の clip 契約で可視性を決める。

今回 retain するのは、背景ピクセルではなく次の CPU 生成物だけである。

- 設定 UI のレイアウト結果 (`InkView` / `GlassSurface` / `TextView`)
- cosmic-text で shape 済みの glyph geometry
- `HitMap`

## 調査結果

### フレーム予算

Apple の Metal performance guide は 60 fps の 1 フレームを 16.67 ms としており、平均だけでなく安定した frame time を重視している。
また、Instruments の Metal 分析ガイドは skipped vsync と CPU/GPU の各区間を分けて確認するよう説明している。

このアプリでは設定表示中も全面 blur を維持するため、GPU の仕事量をゼロにはできない。したがって CPU 側で毎フレーム同一の UI を組み立て直す時間を除き、GPU に渡せる予算と frame-time の余裕を増やす。

### 変更前の CPU ホットパス

`render_settings_panel` は背景しか変化していないフレームでも、以下を再実行していた。

1. immediate-mode `Ui` と複数の `HashMap` / `Vec` を生成
2. Debug カテゴリの全行をレイアウト
3. `TextView` ごとに cosmic-text で計測と shaping
4. glyph quad を再構築
5. `HitMap` を複製
6. renderer model を同じ内容で再設定

renderer 側には同値 batch の GPU upload を省略する dirty tracking が既にある。しかし同値判定に到達する前の CPU レイアウトと文字 shaping は毎フレーム完了していた。

### 一次資料との対応

- Apple の Core Animation guide は、表示内容と表示状態を分離し、位置などの状態変更で同一内容の CPU 再描画を避けるモデルを説明している。今回借りるのはこの「不変な表示モデルを保持する」という考え方だけで、同ガイドの bitmap backing store を背景 blur に適用するものではない。
- cosmic-text の `Buffer` も dirty state を解決して必要な shaping/layout を行う設計であり、不変な文字列を毎フレーム新規 shape する必要はない。
- wgpu の `Queue::write_buffer` は native で staging memory の短命 allocation を伴い得る。既存 dirty tracking と組み合わせ、UI model が不変のフレームでは buffer update 自体を発生させない方針が妥当である。

## 実装

### キャッシュキー

次のどれかが変わった場合だけ UI model を再構築する。

- viewport / DPI
- category、設定値、デバッグフラグ
- open/close progress、sidebar hover/selection animation
- pointer position / pressed state
- page frame geometry
- glyph atlas dimensions

glyph atlas の拡張では UV の分母が変わるため、atlas dimensions をキーに含める。shaping 中に atlas が拡張した場合は、次フレームで必ず再構築される。

### スクロール中

スクロール位置はキャッシュキーに含めない。キャッシュを作った位置を `source`、現在位置を `target` とすると、ScrollView 内の要素だけを次の差分で移動する。

```text
raw_delta_y = source - target
ink/text delta_y = raw_delta_y * panel_visual_scale
glass/hit delta_y = raw_delta_y
```

`clip != None` が ScrollView 内の要素であることを示す。固定のパネル、sidebar、close button、focus veil は移動しない。clip rectangle 自体も viewport に固定する。

スクロールが止まった次の安定フレームでは一度だけ再構築し、停止後の pointer hover を新しい行位置に合わせる。

### HitMap と文字 clip

`HitRegion` に ScrollView の clip を継承させ、スクロール差分では shape だけを移動する。これにより画面外へ出た行が入力を受けない。

設定文字の glyph にも `TextView.clip` を伝播する。従来は文字 shaping 後に clip が失われていたため、スクロール領域外の文字と hit region で visual/input の契約が一致していなかった。

## release 計測

2026-08-13、同一マシン、`--release` で Debug カテゴリを計測した。数値は CPU マイクロベンチであり、全面 blur を含む総 frame time ではない。

```text
cargo test --release profile_retained_settings_scroll -- --nocapture

Immediate rebuild:    1919.547 us/frame
Retained materialize:   23.339 us/frame
```

スクロール中の CPU adapter 処理は、このサンプルでは約 82.2 倍短縮した。静止フレームでは全 materialize も不要で、共有 Overlay lane に設定 toggle lens を再結合する処理だけが残る。

`Immediate rebuild` がより大きいのは、layout だけでなく `TextView` の計測、shaping、glyph geometry 化を含むためである。

## 検証と残課題

- cache hit/miss を debug profiler に追加し、実機操作時に 5 秒窓で確認できる。
- retained 差分、glyph clip、HitRegion clip を unit test で固定する。
- `cargo test --all-targets`、`cargo clippy --all-targets --all-features -- -D warnings`、release build を通す。
- GPU の全面 blur は意図的に未変更なので、総 FPS がまだ不足する場合は Instruments の Display / Metal System Trace と wgpu timestamp query で pass 別 GPU 時間を測る。見た目の契約を変更する最適化は、その測定とは別の設計判断とする。

## 参照

- [Apple: Analyzing the performance of your Metal app](https://developer.apple.com/documentation/xcode/analyzing-the-performance-of-your-metal-app/)
- [Apple: Achieving smooth frame rates with a Metal display link](https://developer.apple.com/documentation/Metal/achieving-smooth-frame-rates-with-a-metal-display-link)
- [Apple: Core Animation Basics](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/CoreAnimationBasics/CoreAnimationBasics.html)
- [Apple: Improving Animation Performance](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/ImprovingAnimationPerformance/ImprovingAnimationPerformance.html)
- [wgpu: Queue::write_buffer performance considerations](https://docs.rs/wgpu/latest/wgpu/struct.Queue.html#method.write_buffer)
- [cosmic-text: Buffer::shape_until_scroll](https://docs.rs/cosmic-text/latest/cosmic_text/struct.Buffer.html#method.shape_until_scroll)
