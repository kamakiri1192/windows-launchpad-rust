# Goal: Apple Liquid Glass Toggleコンポーネントの実装

再利用可能な共通`Toggle`コンポーネントを実装し、macOS 26およびiOS 26の標準Switchに近い外観、操作感、Liquid Glassインタラクションを再現してください。

この実装は設定パネル専用にせず、将来以下でも同じコンポーネントを使用できるようにしてください。

* 設定画面
* 初回起動チュートリアル
* モーダル
* 確認画面
* フォルダー画面
* その他のアプリ内UI

このタスクでは、Toggleの挙動とLiquid Glass表現を実装します。Button、Slider、Segmented Controlなどは対象外です。

## 参照対象

以下を正解の基準として扱ってください。

* iOS 26／iPadOS 26の標準Switch
* macOS 26の標準Switch
* Apple Design ResourcesのiOS 26およびmacOS 26 UI Kit
* WWDC25「Meet Liquid Glass」
* WWDC25「Build a SwiftUI app with the new design」
* SwiftUI／UIKit／AppKitの標準Toggle、UISwitch、NSSwitch

Appleが公開していないアニメーション定数を推測で「完全再現」と断定しないでください。

実装前と実装後に、実機またはSimulator上の標準Switchを録画し、フレーム単位で比較してください。

# コンポーネントAPI

画面側が描画プリミティブ、ヒット領域、アニメーション、Liquid Glass形状を個別に作らなくてよいAPIにしてください。

目標イメージ:

```rust
let response = ui.toggle(
    Toggle::new(settings.liquid_glass_enabled)
        .id("settings.liquid-glass.enabled")
        .label("Liquid Glassを有効化")
        .detail("ガラス効果のマスタースイッチ")
        .style(ToggleStyle::Switch)
        .control_size(ControlSize::Mini)
        .tint(theme.accent),
);

if response.changed {
    settings.liquid_glass_enabled = response.value;
}
```

正確な構文は既存アーキテクチャに合わせて調整して構いません。

最低限、次を指定可能にしてください。

```rust
pub struct Toggle {
    pub id: UiId,
    pub value: bool,
    pub label: Option<String>,
    pub detail: Option<String>,
    pub style: ToggleStyle,
    pub control_size: ControlSize,
    pub tint: Option<Color>,
    pub enabled: bool,
}
```

返り値には最低限、次の情報を含めてください。

```rust
pub struct ToggleResponse {
    pub response: Response,
    pub value: bool,
    pub changed: bool,
}
```

`Response`から、最終配置矩形、ホバー、押下、フォーカスなどを取得できるようにしてください。

# Toggleの構造

Toggleは、論理的には次の要素で構成します。

```text
Toggle
├─ Track
├─ Thumb
├─ Label
├─ Detail
├─ Hit region
└─ Interaction state
```

ただし、Liquid GlassレンダリングではTrackとThumbを別々のガラスレイヤーとして重ねないでください。

操作中のガラス表現は、次のいずれかで単一の協調した構成として描画してください。

* TrackとThumbのSDFを同一のGlass Container内で処理する
* TrackとThumbを単一の統合されたガラス形状として合成する
* Thumbを主となるガラスレンズとし、TrackはTint／Vibrancy／通常Inkで構成する

ガラスの上にさらにガラスを重ねる構成は禁止します。

# 視覚状態

## OFF・静止状態

* Trackは中立色のカプセル形状
* Thumbは先頭側に配置
* 色だけでなくThumb位置でOFFを示す
* Liquid Glass効果は無効または非常に弱くする
* 常時強い屈折、ブラー、シマーを表示しない
* 背景より目立ちすぎない、静かな見た目にする

## ON・静止状態

* TrackへアクセントTintを適用
* Thumbは末尾側に配置
* Tintは単純な不透明ベタ塗りにせず、背景輝度を反映できる色表現にする
* Thumb位置でONを示し、色の違いだけに依存しない
* 静止時のLiquid Glass効果は弱く保つ

デフォルトTintはプラットフォームに適した緑またはアプリのアクセントカラーとします。

## Hover状態

主にmacOSおよびマウスポインター環境向けです。

* TrackまたはThumbの輪郭へ弱いハイライトを表示
* Liquid Glassの完全な操作状態には移行しない
* 形状の拡大は0〜2%程度に抑える
* ON／OFF状態は変更しない
* ポインターが外れたら滑らかに静止状態へ戻す

## Pressed状態

ポインターまたは指が押された瞬間に、待ち時間なしで視覚的反応を開始してください。

* 入力位置を`light_origin`として保存
* 入力位置から内部照明が広がる
* Liquid Glassの強度を素早く上げる
* エッジハイライトを強める
* レンズ効果と屈折を有効化する
* Track／Thumbの協調した形状をわずかに膨張させる
* 押下位置に応じてハイライト中心を移動する

押下反応が遅れて見えないようにしてください。

## Dragging状態

Thumb上、またはToggleの有効ヒット領域内で水平方向へドラッグした場合、直接操作として扱ってください。

* Thumb位置はポインター位置へ1:1に近い形で追従
* Trackの端を超えないようにクランプ
* TrackのTintはThumb位置に応じて連続的に補間
* ON／OFFの見た目を途中で瞬間的に切り替えない
* Thumb移動、Tint、屈折、ハイライトを同じ進行度から計算する
* ドラッグ方向へごく弱いゲル状変形を加えてよい
* 変形量はドラッグ速度と追従誤差に応じて調整する
* 方向を反転した場合は変形方向も滑らかに反転する
* ポインターをToggle外へ動かしても、リリースまではPointer Captureを維持する

ドラッグ開始判定には小さな移動しきい値を設け、通常のクリックと区別してください。

初期値の例:

```rust
drag_threshold = 3.0 logical_px;
```

## Release／Settling状態

リリース時は、ThumbをONまたはOFFの終端へばねで着地させてください。

* 基本判定はThumb中心がTrack中央を越えたかどうか
* リリース速度の影響は、標準Switchとの比較で必要と確認できた場合のみ追加
* 着地時に最大1回の控えめなオーバーシュートを許可
* 複数回振動しない
* Thumb位置、Tint、ガラス強度を同期して収束させる
* Thumbが着地する前にLiquid Glassを突然消さない
* 着地後にLiquid Glass強度を静止状態へ戻す
* 値が変化した場合のみ`changed = true`を返す

# 入力仕様

## タップ／クリック

TrackまたはThumbのどこを押してもToggleを切り替えられるようにしてください。

```text
Pointer down
→ Pressed
→ Pointer up
→ value反転
→ 反対側へSpring移動
→ Settled
```

クリック時にThumbを瞬間移動させないでください。

## ドラッグ

```text
Pointer down
→ Pressed
→ 水平移動がしきい値を超える
→ Dragging
→ Pointer up
→ 最終状態を決定
→ Settling
```

ドラッグ中は値を永続化しないでください。

視覚的なプレビューは連続的に更新して構いませんが、アプリ状態の確定は基本的にリリース時としてください。

## キャンセル

次の場合は操作開始時の状態へ戻してください。

* Pointer Cancel
* ウィンドウフォーカス喪失
* 操作途中でコンポーネントが無効化された
* Pointer Captureが失われた

戻る際も瞬間移動させず、ばねで元の状態へ戻してください。

## キーボード

フォーカス中は、少なくともSpaceキーで切り替えられるようにしてください。

* キー押下中はPressed表現
* キーリリース時に値を切り替える
* マウス操作と同じ着地アニメーションを使う
* 適切なフォーカスリングを表示する

# 状態管理

安定した`UiId`をキーに、一時的な視覚状態を保存してください。

例:

```rust
pub enum ToggleInteractionPhase {
    Idle,
    Hovered,
    Pressed,
    Dragging,
    Settling,
    Disabled,
}
```

```rust
pub struct ToggleVisualState {
    pub thumb_progress: Spring,
    pub press_amount: Spring,
    pub glass_activation: Spring,
    pub tint_progress: Spring,
    pub drag_velocity: f32,
    pub light_origin: Point,
    pub phase: ToggleInteractionPhase,
    pub value_at_press_start: bool,
}
```

`thumb_progress`は次の範囲とします。

```text
0.0 = OFF側
1.0 = ON側
```

アプリのBoolean値と、アニメーション途中の視覚値を分離してください。

# Liquid Glass表現

操作中のToggleでは、最低限次を連動させてください。

* 背景の屈折
* 背景サンプリング座標の変位
* エッジハイライト
* 内部照明
* 背景に応じた影の強度
* 背景に応じた明暗切り替え
* Tint
* 軽い形状変形
* シマー
* ばね運動

Liquid Glassの強度は単純なON／OFFフラグにせず、連続値にしてください。

```rust
glass_activation: f32 // 0.0..=1.0
```

目標:

```text
Idle      ≈ 0.0〜0.15
Hover     ≈ 0.1〜0.25
Pressed   → 1.0
Dragging  ≈ 1.0
Settling  1.0から徐々に減衰
```

正確な値はテーマとして調整可能にしてください。

## 背景適応

Toggleは、明るい背景、暗い背景、色の強い背景のいずれでも判別できる必要があります。

* 小さいガラス要素では背景に応じて明暗を切り替える
* 背景にテキストや細かい模様がある場合は影と分離を強める
* 単純な背景では影を弱める
* Tintを完全な不透明色として描画しない
* ラベルやThumbのコントラストを確保する

設定画面で使うToggleは原則として`Regular`相当の適応型Glassを使用してください。

`Clear`相当のGlassはデフォルトにしないでください。

# サイズ

少なくとも次のサイズをサポートしてください。

```rust
pub enum ControlSize {
    Mini,
    Small,
    Regular,
}
```

表示されるSwitch本体と、実際のヒット領域を分離してください。

* タッチ環境では44×44論理ポイント以上のヒット領域
* macOS相当のポインター環境では28×28論理ポイント程度を標準
* 小さなSwitchでも、見た目を不自然に拡大せずヒット領域だけ確保可能にする

Track、Thumb、余白、ハイライト幅、屈折幅は、短辺を基準に相対的に計算してください。

固定ピクセルだけで調整しないでください。

# アニメーション初期値

以下はAppleの公表値ではありません。実機比較を開始するための調整可能な初期値として実装してください。

```rust
ToggleMotionStyle {
    press_response_ms: 70.0,
    release_glass_fade_ms: 220.0,

    thumb_spring_omega: 24.0,
    thumb_spring_zeta: 0.82,

    press_scale: 1.04,
    hover_scale: 1.01,

    max_directional_stretch: 0.06,
    max_settle_overshoot: 0.04,

    drag_threshold: 3.0,
}
```

要件:

* 60Hz／120Hz／144Hzで感触が大きく変わらない
* 可変`dt`に対応
* 長いフレーム停止後に爆発的な変位を起こさない
* 必要に応じてサブステップを使用
* 最終位置を必ず正確な0.0または1.0へ収束させる

# アクセシビリティ

## Reduce Motion

* ゲル状の伸縮を無効化
* オーバーシュートを無効化
* 移動時間を短縮
* 状態変更自体は認識可能にする
* Thumb位置は瞬時または短いEaseで変更

## Reduce Transparency

* 背景透過を下げる
* より不透明でフロストされた外観へ変更
* ThumbとTrackの境界を維持

## Increase Contrast

* Track、Thumb、背景のコントラストを強化
* 必要に応じて明確な輪郭線を追加
* 主に白または黒を基調とした高コントラスト表現へ変更

## 色覚への配慮

ON／OFFの違いをTintだけで伝えないでください。

最低限、Thumb位置によって状態を判別できる必要があります。

## Disabled

* 入力を受け付けない
* Hover、Pressed、Draggingへ遷移しない
* 状態は判別可能にする
* 不透明度を下げすぎて判読不能にしない

# RenderModelへの統合

Toggleコンポーネントは、内部で以下を一貫して生成してください。

* `GlassSurface`
* `InkView`
* `TextView`
* `HitRegion`
* `UiId`と最終矩形のRegistry登録
* Interaction State
* Toggle Response

画面側から次を直接行わないようにしてください。

* Track用`InkView`の個別push
* Thumb用`InkView`の個別push
* Toggle専用ヒットテストの再実装
* ON／OFF座標の手動計算
* Liquid Glass形状の手動追加
* ホバー／押下アニメーションの個別管理

# GPU実装

各Toggleごとに独立した大型オフスクリーンテクスチャを作らないでください。

可能な限り次を利用してください。

* 既存Liquid Glassの背景テクスチャ
* SDFベースのTrack／Thumb形状
* インスタンス描画
* 共通Glass Container
* 同一背景サンプリング領域
* バッチ化されたコントロール描画

複数のToggleが近接している場合は、同じ背景サンプリング領域を共有できる設計にしてください。

静止状態では低コストなInk中心の描画を使用し、操作中のみLiquid Glass処理を強くする方式を検討してください。

# テスト

以下の単体テストを追加してください。

* OFF状態のThumb位置
* ON状態のThumb位置
* タップによる切り替え
* Track上のクリック
* Thumb上のクリック
* ドラッグ開始しきい値
* 中央を越えたドラッグ
* 中央を越えないドラッグ
* ドラッグ後の逆方向移動
* Pointer Cancel
* Pointer Capture喪失
* 無効状態で値が変わらない
* Spaceキーによる切り替え
* `changed`が値変更時のみtrue
* Thumb位置とTint進行度の同期
* Springが正確な終端へ収束する
* 60Hzと120Hzで最終結果がほぼ一致する
* UiIdから最終矩形を取得できる
* 描画矩形とヒット矩形の整合
* DPI変更時の形状計算
* Reduce Motion
* Reduce Transparency
* Increase Contrast

# 手動QA

iOS 26およびmacOS 26の標準Switchと、同じ大きさ・同じ背景条件で比較してください。

確認する状態:

1. OFF・静止
2. ON・静止
3. Hover
4. Pointer down直後
5. 短いタップ
6. ゆっくりしたドラッグ
7. 高速ドラッグ
8. 中央付近でのリリース
9. ドラッグ中の方向反転
10. 着地
11. キーボード操作
12. Disabled
13. Reduce Motion
14. Reduce Transparency
15. Increase Contrast

背景条件:

* 明るい単色
* 暗い単色
* 明暗差の大きい画像
* 色の強い画像
* テキストが背後を通過する状態
* 動画または動く背景

確認項目:

* 押した瞬間に反応する
* Thumbが直接操作へ追従する
* Tintが途中で急に切り替わらない
* 操作時だけLiquid Glassとして活性化する
* ハイライトが入力位置と対応する
* 屈折が背景に追従する
* リリース時に自然に着地する
* バウンスが過剰でない
* 静止後に揺れ続けない
* ガラス同士が不自然に重ならない
* 100%／125%／150%／200% DPIで破綻しない
* 60Hz／120Hzで印象が大きく変わらない

比較結果は、主観的な「それっぽい」だけで済ませず、録画したフレームを並べて差分を報告してください。

# 非目標

今回は以下を実装しないでください。

* Slider
* Button
* Segmented Control
* Checkbox
* Radio Button
* Toggle Button形式
* チュートリアル画面そのもの
* 公開crate化
* 全画面の一括移行
* Appleの非公開定数を推測して固定値として断定すること

# 完了条件

以下をすべて満たした時点で完了とします。

* 共通`Toggle`コンポーネントとして利用できる
* 設定画面以外でも再利用できる
* 安定した`UiId`を持つ
* IDから最終矩形を取得できる
* タップ、クリック、ドラッグ、キーボード操作に対応する
* Pointer Captureとキャンセルを正しく処理する
* OFF／ONを色以外でも判別できる
* 操作時にLiquid Glassへ活性化する
* 光、屈折、Tint、形状、Thumb移動が連動する
* ガラスの上にガラスを重ねていない
* ばねで自然に終端へ収束する
* アクセシビリティ設定に対応する
* 設定パネルの既存Toggleを新コンポーネントへ移行している
* 既存の手動`toggle_instances`実装を削除または大幅に縮小している
* 描画とヒットテストの座標計算が重複していない
* 単体テストが追加されている
* WindowsおよびmacOSで手動QAされている
* `cargo fmt --check`が通る
* `cargo clippy --all-targets --all-features`が警告なしで通る
* `cargo test`がすべて通る

実装完了後、以下を報告してください。

* Toggleの公開API
* Interaction State Machine
* Track／Thumbの描画構成
* Liquid Glassの活性化方法
* Springと変形のパラメーター
* 入力方法ごとの差異
* アクセシビリティ対応
* GPU負荷とバッチング
* 標準Switchとの比較結果
* 残っている視覚差

