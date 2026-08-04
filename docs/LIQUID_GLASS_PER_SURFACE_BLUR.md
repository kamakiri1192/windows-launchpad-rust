# GlassSurface ごとの背景ブラー

Issue #160 の調査と実装メモ。context menu を最初の対象として、通常の
Liquid Glass と異なる背景ブラー強度を安全に描画する。

## 結論

context menu は、通常面と同じデスクトップキャプチャを入力に使うが、
専用の Dual-Kawase blur chain と専用の full-resolution 出力を持つ。
最終シェーダーは pyramid の途中 level を直接表示せず、各 lane で最後まで
upsample された完成出力だけをサンプルする。

また、Windows の透明ウィンドウでは半透明の blurred RGB を出すだけでは、
DirectComposition/DWM が実デスクトップを再び背後から混ぜる。context menu
は開いている間、形状内の alpha を coverage まで上げ、キャプチャ済みの
blurred desktop で実デスクトップを置き換える。

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
Windows.Graphics.Capture / platform capture
                    |
                    +--> global down/up chain
                    |       -> global full-resolution blur
                    |
                    +--> context-menu down/up chain
                            -> context-menu full-resolution blur

global Glass final pass  ------ samples global completed blur
context menu final pass  ------ samples context completed blur
                                    + backdrop replacement alpha
```

キャプチャは共有する。context menu が表示されている間だけ、同じ workspace
を使って専用 chain を順番に実行する。global の完成出力を書き終えてから
context chain が workspace を再利用するため、二つの final 出力は互いに
上書きされない。

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

`GlassSurface.backdrop_replacement` は、通常の半透明 Glass と、captured
backdrop を実デスクトップの代わりに表示する material を区別する。

- `0`: 従来の半透明 Liquid Glass
- `1`: shape coverage を出力 alpha として使い、DWMのsharp desktopを遮る
- `0..1`: context menu の開閉中に両者を補間する

context menu は `content_opacity` をこの値へ接続する。完全に開いた中央は
alpha 1、角ではSDFのcoverage、閉じる途中ではrevealと一緒に透明へ戻る。
通常の page、folder、control、settings は0のままなので既存合成を維持する。

## 入力ソースの範囲

今回の context menu blur の入力は、既存Liquid Glassと同じデスクトップ
キャプチャである。メニュー直下の実デスクトップはblurred captureで置換される。

完成済みのlauncher scene（アプリアイコン、folder panelなど）全体をnative
backdropのようにぼかす場合は、context menuより前のsceneを別textureへ
flattenし、そのtextureを入力にする追加設計が必要になる。render targetを
同じpassで読み書きすることはできないため、この拡張はdesktop blurとは分ける。

## 更新条件とコスト

- backdrop capture は一回だけでglobal/contextの両方が共有する
- global blur と context blur は別々のdirty flagを持つ
- capture更新時は表示中の両出力を更新する
- context menu radiusのアニメーション中はcontext出力だけを再生成する
- context menu非表示時はcontext chainを実行しない
- context用に追加する常駐full-resolution textureは一枚
- pyramid workspaceは既存のL1/L2/L3を共有する

## テストと視覚QA

自動テストでは次を固定する。

- Rust/WGSL uniform layout
- blur shaderとfinal pipelineのwgpu validation
- 16/24/32pxでkernel scaleが1.0/1.5/2.0になること
- context menuのblur radiusとbackdrop replacementがmodelへ入ること
- blur textureの定数色energyが維持されること

Windows実機では、細かい文字やデスクトップアイコンの上でmenuを開き、次を
確認する。

1. menu中央でデスクトップの輪郭が明確にぼける
2. menu外はsharpなまま変わらない
3. 開閉中にblur強度とreplacement alphaが連続して変化する
4. 角にblack/transparent fringeが出ない
5. DPI変更、resize、GPU/CPU capture fallback後も位置が一致する
6. `disable_blur`時はsharp captureへ戻るが、menu shapeの合成は破綻しない

スクリーンショットを有効にする場合は `docs/EDIT_MODE_VISUAL_QA.md` の手順を
使う。通常起動ではself-captureを防ぐためwindow capture exclusionを維持する。
