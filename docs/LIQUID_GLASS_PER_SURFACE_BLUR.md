# GlassSurface ごとの背景ブラー調査

Issue #160 の調査メモ。目的は、`GlassSurface` ごとに背景ブラー強度を変えたときに、既存の Liquid Glass の描画順・SDF 合成・blur pyramid・backdrop capture を壊さず実現できる粒度を決めること。

## 結論

推奨する実装順は次のとおり。

1. **短期**: context menu を独立した「component/pass 単位」の blur override として実装する。context menu は `GlassLayer::ContextMenu` と専用 shape buffer/final pass をすでに持ち、表示中のガラス面も一枚なので、`content_blur` を実際の shader 入力へ接続する最初の対象に適している。
2. **共通化**: UI モデルには `GlassSurface` の optional な blur override を追加する。ただし `GlassShape` に連続値を直接追加せず、CPU 側で値を少数の `BlurProfile` に量子化し、同じ profile の面をまとめて描画する。
3. **GPU**: backdrop capture は一枚を共有し、最大要求 profile までの dual-Kawase pyramid を一度だけ生成する。final shader は profile に対応する pyramid level を参照する。surface ごとに別の pyramid を作る方式は採用しない。

この方式なら、通常の面は既存の共通値を使い続けられる。異なる blur を要求する面だけが追加の geometry/final fullscreen pass を発生させるため、`GlassSurface` の API と GPU コストを直接結びつけずに済む。

## 現行実装

### UI モデルから GPU まで

| 段階 | 現在の責務 | per-surface blur に関係する制約 |
| --- | --- | --- |
| `ui_model::render_model::GlassSurface` | rect、radius、material、behavior、z、clip、activation、tint を renderer-neutral に保持 | blur のフィールドはない。`GlassLayer` ごとに `GlassBatch` へまとめられる |
| `renderer::prepare` | `GlassSurface` を `GlassShape` へ pack し、layer ごとの shape buffer を更新 | 現在の `GlassShape` は 96 bytes 固定。activation/tint/clip は pack されるが、blur は pack されない |
| geometry shader | shape storage buffer を走査して SDF、displacement、height、alpha、tint を生成 | 一つの pass 内では面を smooth-union する。面ごとの blur の勝者を出力する契約はない |
| blur pass | captured backdrop から L1=1/2、L2=1/4、L3=1/8 を生成し、最後に full-res `blur_texture` へ upsample | pyramid は renderer 全体で共有。深さは共通 `LiquidGlassParams.blur_radius` から決まる |
| final shader | geometry/tint と、backdrop および full-res `blur_texture` をサンプルして合成 | final bind group に blur texture は一つだけ。全面が同じ `u.blur_radius` と同じ blurred backdrop を使う |

対応する主な実装箇所は以下。

- `src/ui_model/render_model.rs`: `GlassSurface`、`GlassLayer`、`GlassBatch`
- `src/renderer/prepare.rs`: `GlassSurface` → `GlassShape` の変換と layer の pass 選択
- `src/liquid_glass/geometry.rs`: 96-byte `GlassShape` の storage layout
- `src/liquid_glass/renderer.rs`: 共通 `LiquidGlassParams`、blur texture/pyramid、per-layer bind group
- `src/liquid_glass/renderer/frame.rs`: capture、blur down/up、geometry pass、final pass
- `assets/shaders/liquid_glass_geometry.wgsl`: SDF と geometry/tint attachment
- `assets/shaders/liquid_glass_final.wgsl`: backdrop/blur のサンプルと最終合成

### blur pyramid の更新条件

`LiquidGlassRenderer::blur_level_count` は、現在の共通 `blur_radius` を capture texture の解像度に合わせて調整し、1/2/3 level を選ぶ。blur の再生成は `blur_dirty || captured` のときだけで、capture が同じフレームでは既存結果を再利用する。

したがって per-surface 化で守るべき性質は次のとおり。

- blur profile の変更だけで backdrop capture を増やさない。
- frame ごとの面のアニメーションで pyramid を再計算しない。
- 現在の最大 blur より深い level を要求する surface がないときは、深い level を省略する。
- CPU fallback capture と GPU shared texture capture のどちらでも同じ profile 選択になる。

## 粒度の比較

| 粒度 | 実装 | GPU コスト | 見た目/保守性 | 判定 |
| --- | --- | --- | --- | --- |
| 全体共通 | 現状の `LiquidGlassParams.blur_radius` | 最小 | 面ごとの iOS 風の差を表現できない | 既存のデフォルトとして維持 |
| layer/pass 単位 | layer ごとに uniform または blur profile を持たせる | 面の種類数に比例。既存 pass を再利用しやすい | context menu、control、modal のような独立 lane と相性がよい | context menu の第一段階に推奨 |
| component 単位 | context menu など特定 feature が専用 blur output を持つ | 対象 component が開いている間だけ追加コスト | 影響範囲が狭く、QA と rollback が容易 | 最初の実装候補 |
| `GlassSurface` 単位 | surface を profile 別に partition し、profile ごとに final pass | distinct profile 数に比例した fullscreen pass。pyramid は共有可能 | overlap の順序を定義できる。shape buffer の ABI 変更を避けられる | 一般化の推奨案 |
| surface ごとの独立 pyramid | 各面が個別の down/up pass と texture を持つ | 面数に比例して非常に高い。VRAM と frame time の悪化が大きい | 単純だがスケールしない | 採用しない |

## 推奨 GPU 設計

### 1. blur の値を profile にする

`GlassSurface` の公開値は `Option<f32>` の override とし、`None` は既存の共通 `LiquidGlassParams.blur_radius` を意味する。renderer の準備段階で次のような有限 profile に変換する。

```text
Sharp  : blur = 0
L1     : 弱い blur（既存 pyramid の浅い level）
L2     : 中程度の blur
L3     : 強い blur
```

初期実装では profile 間を連続補間せず、既存の `blur_radius` → pyramid depth の境界を profile の基準にする。連続値が必要になった場合は隣接 level の線形補間を後続調査とする。これにより、UI のアニメーション中に blur texture を毎フレーム作り直す必要がなくなる。

### 2. surface を profile ごとに分ける

同じ `GlassLayer` に異なる profile の面がある場合、`renderer::prepare` で profile ごとの shape 集合に分ける。各集合は現在と同じ geometry pass/final pass の組で描画し、final shader には対応する pyramid view を bind する。

この案の利点は、現在の 96-byte `GlassShape` の ABI を変更せずに済むこと。blur は shape の属性ではなく、shape buffer と final bind group を束ねる pass state として扱う。

ただし、同じ layer 内で異なる profile の面が重なった場合の順序を明文化する必要がある。初期実装では以下を契約にする。

- 同じ profile の面だけ smooth-union する。
- 異なる profile の集合は `z` と model order に基づく安定した順序で描画する。
- 別 `GlassLayer` の面（例: `Modal` と `ContextMenu`）は既存の layer 順を維持する。
- overlap したピクセルでは後から描画した surface が前の surface を覆う。異なる blur を一つの smooth-union に混ぜない。

### 3. pyramid は最大要求値まで一度だけ作る

blur down/up pass は、表示中の全 surface/profile の最大 level を求めて一回実行する。すでに存在する L1/L2/L3 の texture を final pass から参照できるように bind group を拡張する。追加で必要になるのは主に次のリソース。

- profile または level を選ぶ renderer 内の pass state
- geometry/final pass を profile 数だけ回すための shape buffer/bind group 管理
- final shader の blur texture bindings（sharp は backdrop、blur は既存 pyramid level）

profile を 4 段階に固定するなら、per-pixel の blur selector attachment は不要。これにより、現行 geometry attachment の RGBA16Float の意味（displacement、height、alpha）を崩さず、tint attachment とも競合しない。

### selector attachment 方式を初期案にしない理由

一つの geometry pass の出力に surface ごとの blur selector を追加し、final shader 内で level を選ぶ方式は fullscreen pass 数を増やさずに済む。しかし現行 geometry shader は複数 shape を smooth-union し、tint は「内部で境界に近い shape」を優先しているだけで、surface identity を保持していない。

selector を追加すると、以下の仕様を新たに決める必要がある。

- 異なる blur の shape が smooth-union の橋で重なったときの selector 補間
- nested surface の blur と tint の優先順位
- shape の z と geometry の走査順が一致しない場合の結果
- selector 用の追加 render target の format、clear、capture/QA 表示

面数が増え、profile 数による pass 増加が実測上のボトルネックになった段階で比較する。現状の context menu/Modal/Control の面数では、profile partition の方が回帰リスクを抑えやすい。

## context menu への接続方針

現状は `ContextMenuState::content_blur()` が存在し、`ContextMenuInput` に渡っている。しかし `GlassSurface` に反映されず、`render_context_menu_glass` も `uniforms_from_params(..., activation = 0.0, ...)` を使っており、content blur は shader の blur 入力になっていない。

実装時は次の流れにする。

1. `ContextMenuState` の open/close spring が返す `content_blur` を、context menu layout が生成する `GlassSurface.blur_radius` に渡す。
2. `None`/未対応時は global blur profile にフォールバックし、既存の表示を変えない。
3. renderer が context menu surface を `ContextMenu` lane の profile として partition する。
4. 開くときは iOS 風の目標 blur へ spring で近づけ、閉じるときは content opacity の fade と同じライフサイクルで profile を浅くする。
5. `content_opacity` が可視性閾値を下回ったら、現在と同じく context menu glass pass 自体を空にする。

`content_blur` の目的は背景をぼかすことであり、文字の opacity/scale の代替にはしない。文字の可読性は現行の `content_opacity`、tint、foreground color と独立に QA する。

## コスト見積もりと回帰リスク

### GPU/メモリ

- backdrop capture は共有のまま。surface 数に応じた capture の増加はない。
- 最大 level までの dual-Kawase down/up は一回だけ。profile が既存の最大 level 以下なら blur pass 数は増えない。
- pyramid texture 自体は既存の L1/L2/L3 を再利用する。final bind group の view 数は増えるが、texture allocation は増やさない。
- profile 数が 1 より多い frame は geometry と final の fullscreen pass が profile 数分増える。通常の context menu は一枚なので追加は一組に限定できる。
- CPU 側の shape partition と bind group 切り替えは、アプリ tile 数に比例する SDF 走査に比べて小さい。ただし upload の dirty check は profile ごとに維持する。

### 回帰リスク

- `GlassShape` の storage layout を変更すると Rust/WGSL の ABI 不一致が起きるため、推奨案では変更しない。
- blur profile の pass 順を誤ると、folder/modal/context menu の overlap 順が変わる。既存 `GlassLayer` 順と `z` tie-break をテストで固定する。
- capture region の padding は最大 blur profile を前提にする。profile ごとに capture region を狭めると、端の refraction/blur サンプルが欠ける可能性がある。
- CPU fallback と GPU shared capture で pyramid の texture size が違う場合も、既存 `BackdropMapping` を使って同じ UV 変換を通す。
- debug の `disable_blur`、`show_backdrop_texture`、`show_geometry_texture`、`show_final_glass_only` は全 profile に同じ意味で適用する。
- profile を追加しても、既存の global `blur_radius` の設定・永続化・キーバインドの意味は変えない。

## 実装時のテストと視覚 QA

### 自動テスト

- `GlassSurface.blur_radius = None` が global profile にフォールバックする。
- blur radius が profile 境界（0、6、16 付近）で決定的に量子化される。
- 同一 layer の surface が profile 別に partition され、同じ profile の順序と `z`/model order が保持される。
- context menu の open/close で `content_blur` が surface の profile に伝わり、content opacity が閾値を下回ると glass batch が空になる。
- `GlassShape` の size/alignment と WGSL struct の ABI テストが引き続き 96 bytes を保証する。
- profile が一つだけの frame は既存の single-pass path と同じ pass 数になる。

### 視覚 QA

背景に細かい文字、アイコン、斜めのエッジを含むデスクトップを用意し、次を確認する。

1. 通常の page glass と context menu を同時に表示し、menu 内の背景だけが目標 profile でぼける。
2. menu を開く途中で、背景 blur が spring に追従し、文字だけが先に消えたり背景が急に透明になったりしない。
3. menu を閉じる途中で、collapsed seed の disc が残らず、content opacity の閾値で glass pass が消える。
4. folder panel と context menu が重なる場合、二つの SDF が smooth-union せず、各 panel の境界と blur が保たれる。
5. 0 / 弱 / 中 / 強 profile を切り替えて、色付き背景・アイコン・文字の残像が段階的に変わる。
6. `disable_blur`、capture fallback、ウィンドウ移動、DPI 変更、resize 後でも profile 選択と境界サンプルが破綻しない。
7. GPU self-capture を使う場合は `docs/EDIT_MODE_VISUAL_QA.md` の手順に従い、`LAUNCHPAD_ALLOW_SCREENSHOT=1` と `LAUNCHPAD_QA_SHOT_FILE` を設定して開閉途中のフレームを保存する。

## 今回の PR の範囲

この PR は調査・設計メモの追加に限定する。実際の `GlassSurface.blur_radius` 追加、pyramid view の bind、profile partition、`content_blur` の shader 接続は、上記の自動テストと視覚 QA を伴う後続実装で行う。
