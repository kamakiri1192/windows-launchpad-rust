# GlassSurface ごとの背景ブラー

Issue #160 の調査と実装メモ。context menu を最初の対象として、通常の
Liquid Glass と異なる背景ブラー強度を安全に描画する。

## 結論

context menu は、メニュー直前まで描画した透明swapchainを退避し、それを
デスクトップキャプチャへpremultiplied-alpha合成した完成シーンを入力に使う。
その完成シーンに専用の Dual-Kawase blur chain と専用のfull-resolution
出力を持たせる。
最終シェーダーは pyramid の途中 level を直接表示せず、各 lane で最後まで
upsample された完成出力だけをサンプルする。

また、Windows の透明ウィンドウでは半透明の blurred RGB を出すだけでは、
DirectComposition/DWM が実デスクトップを再び背後から混ぜる。context menu
は開いている間、形状内のalphaをcoverageまで上げ、デスクトップ、Launchpad
アイコン、folder panelを含むblurred sceneで下層を置き換える。

## 修正前の問題

### Pyramid level は blur profile ではない

Dual-Kawase の処理順は次のとおり。

```text
backdrop
  -> down L1 -> down L2 -> down L3
  -> up L2   -> up L1   -> full-resolution blur
```

L1/L2/L3 は処理途中の workspace で、upsample 中に L2 と L1 は上書きされる。
したがって、L1/L2/L3 を「弱・中・強の完成済み blur」として final shader
から直接選ぶことはできない。特に L3 を選んでも、完成出力より強い blur に
なる保証はない。

### 半透明合成で sharp desktop が戻っていた

Liquid Glass の final pipeline は premultiplied-alpha blending を使う。
修正前の context menu 中央 alpha は最大 0.92 だったため、概ね次の合成に
なっていた。

```text
output = blurred capture * 0.92 + previous target * 0.08
```

さらに透明ウィンドウをDWMが実デスクトップへ合成する。このため final
shader 内の sharp sample を除いても、下地または実デスクトップの輪郭が
再混入し、「ぼかし画像を上から足しただけ」の見え方になっていた。

## 現在の描画フロー

```text
desktop capture -----------------> global down/up chain
                                      -> global full-resolution blur

pre-menu transparent swapchain --+
                                  +-> flatten over desktop capture
desktop capture -----------------+      -> opaque completed scene
                                             |
                                             +-> context down/up chain
                                                   -> context full-resolution blur

global Glass final pass -------- samples global completed blur
context menu final pass -------- samples blurred completed scene
                                   + backdrop replacement alpha
```

デスクトップキャプチャは共有する。context menuが表示されている間だけ、
メニューより前の描画をsubmitしてswapchainを退避する。退避したpremultiplied
RGBAをデスクトップへflattenした後、同じpyramid workspaceでcontext chainを
実行する。globalとcontextのfull-resolution出力は別textureなので、workspaceを
順番に再利用しても互いに上書きされない。

## blur radius の扱い

`GlassSurface.blur_radius` は renderer で次の二つへ変換する。

- pyramid depth: 弱い blur は浅く、16px以上は3 levelを使う
- kernel sample scale: `radius / 16` を基準に各 down/up kernel の幅を変える

既定値では通常面の16pxが sample scale 1.0、context menu の24pxが1.5、
opening/closing seed 側の32pxが2.0となる。どの場合も final shader が読むのは
完成済み full-resolution texture であり、途中 level ではない。

低解像度 capture では、texture pixel と画面 pixel の比率を使って radius と
kernel scale を補正する。capture region の padding も最大要求 radius から
算出し、強い profile の外周サンプルが切れないようにする。

## backdrop replacement

`GlassSurface.backdrop_replacement` は、通常の半透明Glassと、完成済みの
pre-menu sceneを下層の代わりに表示するmaterialを区別する。

- `0`: 従来の半透明 Liquid Glass
- `1`: shape coverage を出力 alpha として使い、DWMのsharp desktopを遮る
- `0..1`: context menu の開閉中に両者を補間する

context menu は `content_opacity` をこの値へ接続する。完全に開いた中央は
alpha 1、角ではSDFのcoverage、閉じる途中ではrevealと一緒に透明へ戻る。
通常の page、folder、control、settings は0のままなので既存合成を維持する。

## 入力ソースの範囲

context menu blurの入力には、メニューより前に描画されたすべてのlauncher
layerを含める。対象はpage glass、トップレベルのアプリアイコンと文字、control、
focus veil、folder/settings panel、その内部contentとbadgeである。context menu
自身のglass、項目アイコン、ラベルは入力に含めず、blur後に描画する。

透明swapchainだけではDWMが背後に置くデスクトップRGBを保持していないため、
そのままblurすると半透明部分が暗くなる。そこでswapchainを一度copyし、capture
regionと同じ解像度でデスクトップキャプチャへsource-over合成してalpha 1の
入力を作る。render targetを同じpassで読み書きせず、copy、flatten、blur、
context finalを別usage scopeとして順番にsubmitする。

## 更新条件とコスト

- backdrop captureは一回だけでglobal/contextの両方が共有する
- global blurはcapture/parameterが変わるまで再利用する
- context blurは下層のアイコンやmodal animationを追うため、menu表示中は毎frame生成する
- context menu非表示時はcontext chainを実行しない
- context用にpre-menu scene copy、capture-sized flattened source、完成blurを各一枚持つ
- pyramid workspaceは既存のL1/L2/L3をglobal/contextで順番に共有する

## テストと視覚QA

自動テストでは次を固定する。

- Rust/WGSL uniform layout
- blur shaderとfinal pipelineのwgpu validation
- 16/24/32pxでkernel scaleが1.0/1.5/2.0になること
- context menuのblur radiusとbackdrop replacementがmodelへ入ること
- blur textureの定数色energyが維持されること
- scene flatten shaderとbind group layoutがwgpu validationを通ること

Windows実機では、細かい文字やデスクトップアイコンの上でmenuを開き、次を
確認する。

1. menu中央でデスクトップの輪郭が明確にぼける
2. menu直下のアプリアイコン、文字、folder childが消えずにぼけて残る
3. menu外はsharpなまま変わらない
4. 開閉中にblur強度とreplacement alphaが連続して変化する
5. 角にblack/transparent fringeが出ない
6. DPI変更、resize、GPU/CPU capture fallback後も位置が一致する
7. `disable_blur`時はsharpな完成シーンへ戻り、下層iconが消えない

スクリーンショットを有効にする場合は `docs/EDIT_MODE_VISUAL_QA.md` の手順を
使う。通常起動ではself-captureを防ぐためwindow capture exclusionを維持する。
