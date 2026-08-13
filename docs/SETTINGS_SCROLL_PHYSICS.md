# 設定画面の縦スクロール物理仕様

調査日: 2026-08-13  
対象: 設定画面の連続縦スクロール（トラックパッド／Magic Mouse／ノッチ式マウスホイール）

## 1. 目的

設定画面のスクロールの物理を、**アプリ一覧（ページャー）と同じモデル**にする。
ページャーは`Scroller`（有理rubber曲線 + 可視速度を引き継ぐspring）を使い、
実機で「端のbounceが自然」と確認されている。設定画面だけ`ContinuousScroller`で
別の物理（PR #185で採用したAppKit準拠モデル）に置き換えたところ、
「bounceが弱い／スローモーションになる／勢いが反映されない」という実機フィードバックが
出たため、両者を同じ物理に統一する。

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

### 2.2 AppKit準拠モデル（PR #185）が実機で弱く感じる

PR #185では「macOSのNSScrollViewに近づける」目的で、次のモデルを採用した。

- 端の伸び: 入力forceを`stiffness = 20`で割る線形圧縮 `stretch = force / 20`
- 反発: WebKit由来の指数曲線 `x(t) = (x0 + v0 * 0.31 * t) * exp(-12.5 * t)`
- momentumが端に達したら即反発開始（WebKit `ScrollingEffectsController`準拠）

調査probe（macOS 15.7.7）ではこの前提は確認できた。

- `_NSElasticDeltaForReboundDelta(x) = x / 20`
- `_NSElasticDeltaForTimeDelta(100, 0, 0.1) = 28.65`、`(0, -1000, t)`の最大値
  ≈ 9.1px など、多点サンプリングで指数曲線と完全一致
- `_NSHyperbolicElasticDeltaForReboundDelta`等の双曲線関数はmacOS 15には存在しない

しかし実機レビューでは「強く払った時のbounceが弱く、コンテンツがスクロールされ
切る」「bounceが常にスローモーション」「タッチパッドの勢いが反映されない」と
なった。原因はモデルの数字そのものではなく、**圧縮率と戻り速度の組み合わせ**にある。

- 60pxのmomentum packetが端を越えても表示は3pxしか動かない（force/20）。
  勢いが視覚的に消える。
- 反発のピークは`v0 * 0.31 / (12.5 * e)`程度で、v0=3750px/sでも~35px。
  減衰`exp(-12.5t)`のため戻りに0.5秒かかり、スローモーションに見える。

また、実NSScrollViewに合成momentum eventを送る比較計測（2026-08-13）では、
NSScrollViewはmomentum中ずっとforceを累積し続け（60px packetごとに表示伸びが
ちょうど3px成長）、terminalで反発することを確認した。これはWebKitの
`ScrollingEffectsController`（端到達で即反発+残り無視）とも異なる。

一方、アプリ一覧のページャー（`Scroller`）は表示空間で動く有理rubber曲線と
fast springを使い、実機で自然なbounceとして受け入れられている。

**結論: 設定画面もページャーと同じ物理（表示空間の有理rubber + 可視速度を
引き継ぐspring）に統一する。** NSScrollViewの`x/20`圧縮は「勢いが見えない」、
WebKitの指数反発は「戻りが遅い」ため、どちらも採用しない。

## 3. 情報源の優先順位

1. アプリ内の既存実装（ページャー`Scroller`）の物理と実機での受け入れ状況
2. Apple公開ドキュメント: 入力単位、contact/momentum phaseの意味
3. winit 0.30.13 source: 各OSから`MouseScrollDelta`への変換契約
4. Microsoft Win32 documentation: wheel detentとユーザー設定行数
5. AppKit private関数の調査probe（検算のみ。製品コードからは呼ばない）

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

ページャー（`Scroller`）と同じ構造。`ContinuousConfig`のパラメータも
ページャーの`WheelConfig`と同じ値を使う。

### 5.1 境界内

contact packetは1:1でpositionへ適用する。native momentum packetは
**端に届かないパケットだけ`momentum_gain`（既定 0.4）を掛け**、端を越える
パケットは**境界内成分も含めてフルフォース**でrawへ累積する。OSがすでに
momentum packet を生成しているため、momentum終了後にアプリ独自の惰性を
追加しない。

```text
in_bounds = delta.clamp(-room, room)      （room = 現在のrawから境界までの余裕）
crosses   = (delta - in_bounds) != 0
applied   = crosses ? delta : in_bounds * momentum_gain
raw      += applied
position  = rubber(raw)
```

`momentum_gain`の理由: AppKitのmomentumはシステムのスクロール加速設定を
反映するため、設定が速いと中程度のフリックでも内容が端まで飛んでbounceする
（実機トレース 2026-08-13: 130pxのフリックが972px coast、-362pxのフリックが
750px coast）。ページャー（`Scroller`）はnative momentumをrouterで隔離して
自前の減衰を使うため環境に依存しない。設定パネルも同じ感覚のcoastに揃える
ため、境界内の移動距離を約40%にスケールする。

**端を越えるパケットにはゲインを掛けない。** ゲインを全体に掛けると、端まで
到達したmomentumが境界を越えられず「ピタッと止まる」見た目になる（stretch
も反発も発生しない）。境界を越えるパケットはその全体をフルフォースにすることで、
十分強いフリックは従来どおり端でストレッチして反発する。

### 5.2 境界外への伸び

**アンカー型**。ジェスチャ開始時にraw座標をアンカーし、各packetのdeltaをrawへ
累積してからrubberを適用する。**伸びは浅く**、漸近は viewport の約7.5%で、
bounce は「深い引っ張り」ではなく「素早いひと押し」として見える。

```text
raw   = アンカー + Σ delta        （アンカー = 表示位置のraw相当）
position = rubber(raw)

rubber: u = |raw - boundary| / viewport
        stretch = viewport * rubber_max_pages * u / (rubber_curve_a + u)

rubber_max_pages = 0.075
rubber_curve_a    = 0.30
```

100pxの入力で約14px（viewport 400px時）、漸近は約30px（viewport 400px時）。
実機レビューで「ページャーと同じ深さ（20%）は深すぎる」と判断され、5〜10%の
範囲の7.5%に調整した。

有理曲線の漸近近くでは表示の動きが小さくなるため、長いmomentumの終盤は
**momentum尻尾の打ち切り**（§5.4）で早めに反発へ移り、伸びたまま止まって見える
時間を短くする。

**「表示位置にdeltaを足してからrubber」という増分方式は使わない。** 実機トレース
（2026-08-13）で、深い伸びの状態でcoalescedな大きいpacket（例: 33ms間隔・
-180px）が来ると、圧縮済みの表示位置をrawと誤って足すため、外側へ押しているのに
表示が内側へ跳ね返る（例: -83.13 → -65.89）「ビクビク」が発生した。アンカー型なら
表示は常に`rubber(累積raw)`なので単調に深くなり、coalescingの影響も受けない。

逆向きの入力も同じraw空間に足すため、rawが境界内へ戻ればそのまま1:1になる
（ページャーと同じ挙動）。

### 5.3 反発

接触の終了（contact `Ended`）またはmomentumの終了（`Momentum Ended` /
`Cancelled` / `sequence_complete`）で、境界外ならSettlingへ移る。

```text
spring_omega = 15  (rad/s, ≈ 2.4 Hz)
spring_zeta  = 0.75 (underdamped → soft, gently oscillating return)
```

初速は**表示位置のサンプルから推定した可視速度**をそのままspringへ渡す
（ページャーの`begin_wheel_settle`と同じ）。サンプルの時刻はネイティブeventの
timestampを使い、フレーム間の壁時計に依存しない。contact→momentumの境界で
サンプル履歴を消さないため、途中から強く払って端へ到達したflingの速度が
反発に残る。

critical damping（ζ=1.0）は「カチッと」戻る硬い印象だったため、ζ=0.75の
不足減衰にしてオーバーシュートしながら柔らかく戻るようにした。ωは
`2π·f` で f≈2.4 Hz。60/120/144Hzで軌道が一致するよう、Settling は厳密解
（critical damping は `(x0 + (v0 + ω*x0)*t) * exp(-ω*t)`、underdamped は
減衰振動の解析解）で更新する。

### 5.4 phase遷移

```text
Contact Began   -> Tracking（進行中のsettleを中断して現在位置を引き継ぐ）
Contact Changed -> 1:1 scroll / edge stretch
Contact Ended   -> edge外ならSettling（可視速度を継承）、範囲内ならIdle
Contact Cancel  -> edge外なら速度0でSettling

Momentum Began/Changed
                -> 端に届かないパケットは境界内成分 × momentum_gain、
                   端を越えるパケットは全体をフルフォースでrawへ累積
                   （表示は有理rubberで圧縮）
                -> Trackingのまま。Reboundは開始しない
Momentum Ended  -> edge外ならSettling（累積した伸びと可視速度で反発）、
                   範囲内ならIdle
```

Momentumは直前のcontactの継続なので、`Momentum Began`で速度サンプルを初期化
しない。指を離す直前のpacketから最初のmomentum packetまでの実時間で速度を求め、
途中から強く払って端へ到達した場合も、そのlaunch velocityを反発へ渡す。

新しいcontactは進行中のsettleを中断し、表示中の位置から連続して操作を引き継ぐ。

## 6. マウスホイール物理モデル

- ノッチ入力は疎なため、目的移動量を指数減衰impulseへ変換してフレーム間を補間する。
- impulseの積分距離は入力で指定された総pxと一致させ、補間で移動量を減らさない。
- 近接したノッチはvelocityへ加算し、連続した操作として見せる。
- 離散ホイールは端を越えてrubber-bandさせず、境界で停止する。
- macOSでは`scrollingDeltaY * row_height`、WindowsではOS設定行数も掛ける。
- pointer fling用の`3.2/s`とは分離し、離散ホイールは`14/s`で減衰する。これにより
  1 stepの90%を約165msで表示し、総移動量は変えずに長い慣性だけを除く。この値は
  Apple非公開定数とは主張せず、実機レビューに基づくアプリ側の補間時間である。

## 7. スクロールバー

content positionが境界外なら、thumbを境界へ固定したままoverscroll量に応じて短くする。
track外へthumbを移動しない。反発中もcontentと同じフレームで更新する。

## 8. 受け入れ条件

### トラックパッド

- top/bottomで100px相当の外向き入力を与えたとき、表示伸びは約36pxになる
  （viewport 400px時。ページャーと同じ有理曲線）。
- momentum packetが端を越えた後も、外向きpacketは表示伸びを深くしていく。
  60px packetなら毎回約3px増（AppKit x/20時）ではなく曲線に従って増える。
- 端を越えた状態から境界方向へ戻すと、rawが境界内に入った時点から1:1で効く。
- contact `Ended`の同じframeでSettlingへ入り、可視速度がspringに引き継がれる。
- momentumの終端までRebound（Settling）を開始せず、終端で累積した伸びと
  可視速度から反発する。
- contactからmomentumへの切り替えで速度履歴を失わず、途中から強く払ったときも
  反発の初速が0にならない。
- 端から100px、速度0でsettleを開始した場合、約0.3秒で境界へ収束する
  （critical damping、ω=20）。スローモーションにならない。
- 60/120/144Hzで同じelapsed timeの位置が一致する。

### マウス

- macOSの非精密1 lineは1 settings row相当へ変換される。
- Windowsの1 detentはOS設定が既定値3なら3 settings rows相当になる。
- 同じ総入力の最終移動距離はフレームレートに依存しない。
- 1 stepの90%以上が180ms以内に表示される。
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

### 調査に使用した実装・計測（2026-08-13）

- WebKit `ScrollingEffectsController.mm`（現行。端到達で即反発+残りmomentum無視。
  本モデルとは意図的に異なる）
- AppKit private関数の調査probe（`_NSElasticDeltaForReboundDelta` = x/20、
  `_NSElasticDeltaForTimeDelta` = 指数曲線。macOS 15.7.7で多点サンプリング確認。
  製品コードからは呼ばない）
- 実NSScrollViewへの合成momentum event送信（端を越えてもmomentum中は伸びが
  累積し、terminalで反発することを確認）

### 入力backend / Windows

- [winit 0.30.13 macOS wheel conversion](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/macos/view.rs)
- [winit 0.30.13 Windows wheel conversion](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/windows/event_loop.rs)
- [MouseScrollDelta](https://docs.rs/winit/0.30.13/winit/event/enum.MouseScrollDelta.html)
- [Microsoft: About Mouse Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input)
- [Microsoft: Using Mouse Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/using-mouse-input)
- [SystemParametersInfo](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfoa)
