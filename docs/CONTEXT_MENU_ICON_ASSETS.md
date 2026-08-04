# コンテキストメニューのアイコン追加

コンテキストメニューにブランドロゴなどのSVGアイコンを追加する場合は、SVGを実行時に直接読むのではなく、ビルド前に透過PNGへ変換してから既存のテクスチャ描画経路へ組み込む。

現在のChatGPTアイコンはこの方式で追加されている。

```text
SVG
  -> currentColorをメニュー文字色へ解決
  -> rsvg-convertで256x256の透過RGBA PNGへ変換
  -> assets/icons/ に配置
  -> include_bytes! でバイナリへ埋め込み
  -> wgpuテクスチャとしてアップロード
  -> context-menu用のControlKind / WGSL kindで描画
```

## なぜPNGへ変換するのか

既存の実行時画像デコードは `image` クレートを使っており、PNG、BMP、JPEG、ICOを対象としている。SVGを実行時に扱うための `resvg` や `usvg` は使っていない。

また、通常のコンテキストメニューアイコン（鉛筆、目、フォルダなど）は `shader_control.wgsl` の手書きSDFで描画されている。複雑なブランドロゴをSDFで再現すると形状が崩れやすいため、ロゴは専用のラスター画像として描画する。

## SVGからPNGを作る

macOSでは、まずlibrsvgをインストールする。

```sh
brew install librsvg
which rsvg-convert
```

元のSVGは生成元として `assets/icons/<icon-name>.svg` に保存する。SVGが `fill="currentColor"` を使っている場合は、色を解決した一時ファイルを作る。

```sh
mkdir -p assets/icons
sed 's/currentColor/#1C1C1E/g' \
  assets/icons/<icon-name>.svg \
  > /tmp/<icon-name>-colored.svg

rsvg-convert \
  -w 256 -h 256 \
  /tmp/<icon-name>-colored.svg \
  -o assets/icons/<icon-name>.png
```

`#1C1C1E` は現在のコンテキストメニュー文字色に合わせた値である。ロゴ本来の色を使う場合やSVGがすでに明示的な色を持つ場合は、`sed` による置換は行わない。

生成物を確認する。

```sh
file assets/icons/<icon-name>.png
# PNG image data, 256 x 256, 8-bit/color RGBA ... になっていること
```

背景が白くなった場合は、ImageMagickのSVG内部レンダラーではなく `rsvg-convert` を使う。コンテキストメニューのテクスチャはアルファチャンネルを使って形状を抜くため、RGB画像ではなく透過RGBA PNGにする。

## Rust側への取り込み

### 1. アセットを埋め込む

`src/renderer/init.rs` でPNGを `include_bytes!` し、`image::load_from_memory(...).to_rgba8()` でデコードする。専用の `wgpu::Texture` を作り、`queue.write_texture` でRGBAデータを書き込む。

ChatGPTロゴの実装例は次の場所にある。

- PNGの埋め込みとテクスチャ作成: `src/renderer/init.rs`
- 元SVGと生成PNG: `assets/icons/chatgpt-logo.svg` / `assets/icons/chatgpt-logo.png`

テクスチャ、`TextureView`、`Sampler` は `Renderer` に保持する。ローカル変数のままにすると、bind groupが参照しているリソースが早く解放される。

### 2. bind groupへテクスチャを追加する

`src/renderer/init.rs` のcontrol用bind group layoutへ、次の2つを追加する。

- binding `3`: `TextureView`
- binding `4`: `Sampler`

`src/shader_control.wgsl` でも同じ番号を宣言する。

```wgsl
@group(0) @binding(3) var icon_texture: texture_2d<f32>;
@group(0) @binding(4) var icon_sampler: sampler;
```

重要なのは、グリフアトラス拡張時にbind groupを再構築する `rebind_text_atlas` もbinding `3` と `4` を含めることである。layoutが5 bindingなのに再構築時に3 bindingしか渡すと、`create_bind_group` のvalidation panicになる。

### 3. `ControlKind` とWGSLのkindを追加する

次の3箇所を同じ番号で更新する。

1. `src/ui_model/render_model.rs` に新しい `ControlKind` variantを追加
2. `src/renderer/controls.rs` でvariantを数値kindへ変換
3. `src/shader_control.wgsl` にテクスチャをサンプリングする分岐を追加

ChatGPTロゴは `kind 19` を使っている。テクスチャのアルファをcoverageとして使い、インスタンスのalphaを掛けて最終色を作る。

### 4. UVの範囲をクワッド全体に合わせる

コンテキストメニューのアイコンは、SDFの余白を含めるため頂点シェーダー側で `size * 1.2` のhalf-extentを使う。フラグメントシェーダーのUVも同じ値を使う。

```wgsl
let half_extent = max(in.params.x * 1.2, 1.0);
let uv = vec2<f32>(
    p.x / half_extent + 0.5,
    p.y / half_extent + 0.5,
);
```

`size` だけでUVを計算するとテクスチャがクワッド中央の約83%にしか広がらず、他のアイコンより小さく見える。

## メニューリストへ追加する

テクスチャを用意しただけではメニューに表示されない。次の順番で更新する。

1. `src/layout/context_menu.rs` の `ContextMenuItem` にvariantを追加
2. `ALL`、アプリ用の行数、ラベル配列、`label()`、`item_icon_kind()` を更新
3. フォルダには不要なアプリ専用項目なら、フォルダ用配列から除外
4. `src/app/render/context_menu.rs` の行番号解決と `ContextMenuSelection` を更新
5. 必要なら `src/app/event.rs` と `src/app/command.rs` にコマンドを追加
6. 固定長のfocus配列、行番号、アイコン順序を前提にしたテストを更新

アイコンの順番を変えた場合は、行番号を直接使っているテストも確認する。特に次のようなテストは壊れやすい。

- メニューアイコンの順序を検証するテスト
- `focus_amounts` など固定長配列を使うテスト
- 特定の行番号を特定のアクションとして解決するテスト
- フォルダーでは表示されない行を検証するテスト

## 確認コマンド

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

実際の表示確認では、起動してアプリタイルを右クリックし、次を確認する。

- アイコンがメニュー文字色と合っている
- 背景が白くならず、透明部分が抜けている
- ほかのメニューアイコンと同じ大きさに見える
- グリフアトラスが拡張された後もアイコンが消えたりpanicしたりしない

Liquid Glassのスクリーンショットを撮る場合は、通常起動ではなく
[EDIT_MODE_VISUAL_QA.md](EDIT_MODE_VISUAL_QA.md) に記載されたスクリーンショット許可フラグを設定して起動する。
