# 設定画面の縦スクロール物理仕様

調査日: 2026-08-13  
対象: 設定画面の連続縦スクロール（トラックパッド／Magic Mouse／ノッチ式マウスホイール）

## 1. 目的

設定画面のスクロールを、単に「滑らかな補間」にするのではなく、macOSの
`NSScrollView`に近い入力単位、端の伸び、反転、反発開始タイミングにする。

既存の`docs/TRACKPAD_PAGING_SPEC.md`はLaunchpadページ切り替え用の横方向
ページング仕様である。設定画面はページスナップを持たない連続縦スクロールなので、
入力契約と物理モデルを本資料で分離する。

## 2. 調査で確定した不具合

### 2.1 マウスの移動量が小さすぎる

macOSネイティブ入力経路は、`hasPreciseScrollingDeltas == false`の
`scrollingDeltaY`をscale factorだけで物理pxにしていた。

Appleは、非精密デルタを使う場合は戻り値に「行またはrowの高さ」を掛け、精密
デルタの場合だけ戻り値をそのまま使うよう明記している。winit 0.30.13のmacOS
backendも同じ規則で、非精密入力を`LineDelta`、精密入力をDPI変換した
`PixelDelta`として渡す。本アプリのネイティブmonitorだけがこの変換を迂回していた。

したがって、macOSの非精密入力は次で変換する。

```text
physical_delta = scrollingDeltaY * settings_row_height_physical
```

Windowsでは事情が異なる。winit 0.30.13は`WM_MOUSEWHEEL`の
`wheelDelta / WHEEL_DELTA`をそのまま`LineDelta`にし、Windowsの「一度に
スクロールする行数」は掛けない。Windows版は`SPI_GETWHEELSCROLLLINES`を読み、
通常は次で変換する。

```text
physical_delta = detents * system_scroll_lines * settings_row_height_physical
```

`WHEEL_PAGESCROLL`の場合は1 viewport相当を使う。設定値が0なら移動しない。

### 2.2 トラックパッドの端の挙動がAppKitと異なる

旧実装には次の差があった。

- 指を離した`phase == Ended`では反発を始めず、後続momentumの終了まで待っていた。
- momentumが端へ到達しても、momentumの全packetを受け終えるまで反発しなかった。
- 端の伸びに、iOS向けとして広く紹介される`c = 0.55`の有理式を使用していた。
- 反発に一般的なcritical springを使用していた。
- 端から逆向きへ動かしたときも、圧縮前の累積座標へ入力を足していた。

WebKitのmacOS実装は異なる。

- 物理接触の`Ended`で、伸びがあれば直ちにrubber-band animationを始める。
- native momentumが端に達して反発を開始できる状態になったら、その後のmomentumを
  無視してrubber-band animationへ移る。
- 伸びている間はunaccelerated deltaを使う。
- 現在の伸びと逆向きのdeltaは減衰せず1:1で通し、stretch forceをリセットする。
- 反発にはAppKitの`_NSElasticDeltaForTimeDelta`と同じ軌道を使う。

状態遷移の差が大きいため、ばね係数だけを調整してもmacOSらしくならない。

## 3. 情報源の優先順位

1. Apple公開ドキュメント: 入力単位、contact/momentum phase、elasticityの意味
2. Appleが保守するWebKit: `NSScrollView`へ合わせるための状態遷移と式
3. winit 0.30.13 source: 各OSから`MouseScrollDelta`への変換契約
4. Microsoft Win32 documentation: wheel detentとユーザー設定行数
5. ローカル比較計測: 公開資料に値がない箇所の検算（製品コードからprivate SPIは呼ばない）

ブログ等の二次情報だけを根拠に「Appleの係数」と断定しない。

## 4. 入力契約

| 入力 | 識別 | 単位 | OS momentum | 端のelasticity |
|---|---|---:|---|---|
| macOS trackpad / precise Magic Mouse | `hasPreciseScrollingDeltas == true` | physical pxへ変換 | `momentumPhase`をそのまま使用 | 有効 |
| macOS generic wheel | `hasPreciseScrollingDeltas == false` | row数 × row高 | なし | 無効 |
| Windows wheel | winit `LineDelta` | detent × OS行数 × row高 | なし | 無効 |
| winit `PixelDelta` fallback | `PixelDelta` | physical px | collapsed phaseのみ | phaseが有効なら有効 |

符号変換は入力adapterと`ContinuousScroller`の境界で1回だけ行う。AppKitの
`scrollingDeltaX/Y`はnatural scrolling設定を反映済みなので、設定値を理由に
deltaをもう一度反転しない。

## 5. トラックパッド物理モデル

### 5.1 境界内

contactおよびnative momentum packetは1:1でpositionへ適用する。OSがすでに
momentum packetを生成しているため、momentum終了後にアプリ独自の惰性を追加しない。

### 5.2 境界外への伸び

macOS 15系でWebKitが使用してきたAppKit互換モデルは、入力force `F`を
stiffness 20で割る。

```text
stretch(F) = F / 20
force(stretch) = stretch * 20
```

これはApple WebKit由来の公開実装にある`rubberbandStiffness = 20`と一致する。
調査機のmacOS 15.7.7でも、AppKitの関数を一時的な調査probeから読むと
`_NSElasticDeltaForReboundDelta(x) = x / 20`で一致した。private SPIは調査時の
検算にだけ使い、製品からは呼ばない。

同じ向きのdeltaは`F`へ累積する。逆向きのdeltaはWebKitと同様に減衰せず、表示
positionへ1:1で適用する。これにより、端から戻す指の動きが「圧縮された入力空間」へ
吸収される粘りを防ぐ。

### 5.3 反発軌道

Apple WebKit由来の公開実装にある定数を使用する。

```text
stiffness = 20
amplitude = 0.31
period    = 1.6
lambda    = stiffness / period = 12.5

x(t) = (x0 + v0 * amplitude * t) * exp(-lambda * t)
```

`x0`は境界からの表示上の伸び、`v0`は最後の有効な入力deltaから得る入力速度で
ある。WebKitは100ms以上packet間隔が空いた速度を0とみなす。

終了条件は、開始後24msを超え、境界からの距離が0.5 physical px未満になった時と
する。計算はelapsed timeから直接行い、60/120/144Hzで軌道を変えない。

### 5.4 phase遷移

```text
Contact Began   -> Tracking（進行中の反発を中断して現在位置を引き継ぐ）
Contact Changed -> 1:1 scroll / edge stretch
Contact Ended   -> edge外なら直ちにRebound、範囲内ならIdle
Contact Cancel  -> edge外なら速度0でRebound

Momentum Began/Changed
                -> native deltaを適用
                -> 境界内から端をまたぐpacketは、まず端へclampしてTrackingを継続
                -> 端にpinされた状態で次の外向きpacketを受けたらstretchしてRebound
                -> 残りのmomentum packetをterminalまで無視
Momentum Ended  -> edge外ならRebound、範囲内ならIdle
```

Momentumは直前のcontactの継続なので、`Momentum Began`で入力速度のtimestampを
初期化しない。指を離す直前のpacketから最初のmomentum packetまでの実時間で速度を
求め、途中から強く払って端へ到達した場合も、そのlaunch velocityを反発へ渡す。

新しいcontactは進行中の反発と「momentum無視」状態を解除し、表示中の位置から
連続して操作を引き継ぐ。

### 5.5 将来のAppKit双曲線モデルについて

2026年7月のWebKit変更は、対応OSで次のAppKit private SPIを使用する。

```text
_NSHyperbolicElasticDeltaForReboundDelta(force, coefficient, viewport)
_NSHyperbolicReboundDeltaForElasticDelta(stretch, coefficient, viewport)
```

形はviewport依存の双曲線だが、実トラックパッド用`coefficient`の取得処理は
`WebKitAdditions/ScrollingEffectsControllerAdditions.mm`（Apple内部）にあり、公開
sourceだけでは値を確定できない。またprivate SPIを製品から呼ぶべきではない。

よって今回の基準実装は、公開WebKit sourceとmacOS 15.7.7で一致を確認できる
stiffness 20モデルとする。対応対象のmacOSが新しい双曲線モデルへ移行した場合は、
実機trace fixtureを採取して公開式へfitする。`c = 0.55`を根拠なくAppleの現行値と
みなして置き換えない。

## 6. マウスホイール物理モデル

- ノッチ入力は疎なため、目的移動量を指数減衰impulseへ変換してフレーム間を補間する。
- impulseの積分距離は入力で指定された総pxと一致させ、補間で移動量を減らさない。
- 近接したノッチはvelocityへ加算し、連続した操作として見せる。
- 離散ホイールは端を越えてrubber-bandさせず、境界で停止する。
- macOSでは`scrollingDeltaY * row_height`、WindowsではOS設定行数も掛ける。

## 7. スクロールバー

content positionが境界外なら、thumbを境界へ固定したままoverscroll量に応じて短くする。
track外へthumbを移動しない。反発中もcontentと同じフレームで更新する。

## 8. 受け入れ条件

### トラックパッド

- top/bottomで100px相当の外向き入力を与えたとき、表示伸びは約5pxになる。
- 同じ外向き入力はforce空間で累積する。
- 伸びと逆向きの入力は表示位置へ1:1で効く。
- contact `Ended`の同じframeでRebound状態へ入る。
- 境界内から端をまたぐmomentum packetは端へpinされ、次の外向きpacketで伸びて
  Reboundへ入り、残りのmomentumで再び引き伸ばされない。
- contactからmomentumへの切り替えで速度履歴を失わず、途中から強く払ったときも
  反発の初速が0にならない。
- `x0 = 100, v0 = 0`の反発は100ms後に約28.65pxとなる。
- 60/120/144Hzで同じelapsed timeの位置が一致する。

### マウス

- macOSの非精密1 lineは1 settings row相当へ変換される。
- Windowsの1 detentはOS設定が既定値3なら3 settings rows相当になる。
- 同じ総入力の最終移動距離はフレームレートに依存しない。
- 境界でoverscrollしない。

### 共通

- contentがviewport以下ならscrollもbounceもしない。
- zero-delta terminal packetで位置が飛ばない。
- 新しいcontactで進行中の反発を中断しても位置が飛ばない。
- natural scrollingの符号を二重反転しない。

## 9. 参照資料

### Apple

- [NSEvent.scrollingDeltaY](https://developer.apple.com/documentation/appkit/nsevent/scrollingdeltay)
- [NSEvent.hasPreciseScrollingDeltas](https://developer.apple.com/documentation/appkit/nsevent/hasprecisescrollingdeltas)
- [NSEvent.momentumPhase](https://developer.apple.com/documentation/appkit/nsevent/momentumphase)
- [Handling Trackpad Events](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/HandlingTouchEvents/HandlingTouchEvents.html)
- [NSScrollView.verticalScrollElasticity](https://developer.apple.com/documentation/appkit/nsscrollview/verticalscrollelasticity)

### WebKit / Chromium（公開source）

- [WebKit ScrollingEffectsController.mm](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/platform/mac/ScrollingEffectsController.mm)
- [WebKit ScrollAnimationRubberBand.mm](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/platform/mac/ScrollAnimationRubberBand.mm)
- [WebKit NSScrollViewSPI.h](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/PAL/pal/spi/mac/NSScrollViewSPI.h)
- [2026 WebKit: Scroll stretch behavior should more closely match NSScrollView](https://github.com/WebKit/WebKit/commit/ce76e2b79384d3b751287f8c8da007d3c7fd9fc4)
- [Apple WebKit由来の旧rubber-band実装](https://alioth-lists-archive.debian.net/pipermail/pkg-webkit-commits/2011-February/033152.html)
- [Chromium InputScrollElasticityController](https://chromium.googlesource.com/chromium/src/+/73c2ef2bee31adf9d44827e6ef071a6b8d653bcc/ui/events/blink/input_scroll_elasticity_controller.cc)

### 入力backend / Windows

- [winit 0.30.13 macOS wheel conversion](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/macos/view.rs)
- [winit 0.30.13 Windows wheel conversion](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/windows/event_loop.rs)
- [MouseScrollDelta](https://docs.rs/winit/0.30.13/winit/event/enum.MouseScrollDelta.html)
- [Microsoft: About Mouse Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input)
- [Microsoft: Using Mouse Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/using-mouse-input)
- [SystemParametersInfo](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfoa)
