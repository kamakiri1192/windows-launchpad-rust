# Goal

Issue #122の対応をきっかけに、現在の設定パネル専用・手動構築中心のUI実装を、将来ほかの画面やチュートリアルでも再利用できる、小さな内部UIコンポーネント基盤へ移行してください。

対象リポジトリ:

* `kamakiri1192/windows-launchpad-rust`
* Issue: `#122 設定パネルのピクセル単位滑らかなスクロール`
* 前提: PR #121の設定パネル実装をベースにする

今回の目的は、単に設定パネルのスクロールを滑らかにすることではありません。

今後、Liquid Glassを使用したボタン、トグル、スライダー、スクロール領域、チュートリアルUIなどを、各画面で描画プリミティブやヒット領域を個別に手書きせず、再利用可能なコンポーネントとして配置できる構造にしてください。

最終的には、画面側が次のようなイメージでUIを構築できる状態を目指します。

```rust
ui.scroll_view(&mut state.scroll, |ui| {
    ui.toggle(
        Toggle::new("Liquid Glassを有効化", state.enabled)
            .id("settings.liquid-glass.enabled")
            .detail("ガラス効果のマスタースイッチ"),
    );

    ui.slider(
        Slider::new(state.thickness, 6.0..=48.0)
            .id("settings.liquid-glass.thickness")
            .label("厚み"),
    );

    ui.button(
        Button::new("アイコンキャッシュを再構築")
            .id("settings.reset-cache")
            .style(ButtonStyle::Prominent),
    );
});
```

APIの正確な構文は既存設計に合わせて変更して構いませんが、画面側が`InkView`、`TextView`、`GlyphView`、`HitRegion`、座標計算を個別に組み立てなくてよいことを目標としてください。

# 現状の課題

現在は、レンダラーと`RenderModel`はある程度抽象化されていますが、UIウィジェットは抽象化されていません。

設定パネルでは、各コントロールについて以下を個別に構築しています。

* 背景用`InkView`
* トグルのトラックとノブ
* スライダーのトラック、ノブ、リセットアイコン
* テキスト
* ヒット領域
* 座標計算
* スクロール位置補正
* 表示範囲外の手動フィルタ

描画とヒットテストで同じ座標計算を別々に行っており、コンポーネント追加時の変更箇所も多くなっています。

今後Liquid GlassのUIを増やす場合、この構造のままでは画面ごとの重複が増えるため、今回コンポーネント層を導入してください。

# 設計方針

## 1. アプリ内で再利用できる内部UI基盤にする

設定画面専用の部品にはしないでください。

少なくとも将来、以下から同じコンポーネントを利用できる設計にしてください。

* 設定画面
* 初回起動チュートリアル
* フォルダー画面
* 検索結果
* モーダル
* 確認ダイアログ
* その他のLiquid Glass UI

ただし、今回の段階で別リポジトリの汎用UIフレームワークを作る必要はありません。

まずはこのリポジトリ内の内部UIモジュールとして実装してください。

例:

```text
src/ui/
├── context.rs
├── response.rs
├── interaction.rs
├── theme.rs
├── layout/
│   ├── row.rs
│   ├── column.rs
│   └── scroll_view.rs
├── widgets/
│   ├── button.rs
│   ├── toggle.rs
│   ├── slider.rs
│   ├── label.rs
│   └── divider.rs
└── material/
    └── liquid_glass.rs
```

既存のモジュール構成に合う、より自然な分割がある場合は調整して構いません。

## 2. ウィジェットの挙動とLiquid Glassの見た目を分離する

以下のようなLiquid Glass専用ウィジェットを大量に作らないでください。

```rust
LiquidGlassButton
LiquidGlassToggle
LiquidGlassSlider
```

ウィジェットの操作と、見た目・マテリアルは分離してください。

```rust
Button
Toggle
Slider
ScrollView
```

に対し、スタイルまたはマテリアルとしてLiquid Glassを指定できる構造にしてください。

例:

```rust
Button::new("次へ")
    .style(ButtonStyle::Prominent)
```

```rust
ButtonStyle {
    material: Material::LiquidGlass,
    ...
}
```

将来Liquid Glassのレンダリング方式を変更しても、各画面のUI記述を変更せずに済む設計にしてください。

## 3. 各コンポーネントに安定したIDを持たせる

将来のチュートリアルで、特定コンポーネントをハイライトしたり、自動スクロールしたりできるようにしてください。

各コンポーネントには安定した`UiId`を設定できる必要があります。

例:

```rust
UiId::new("settings.liquid-glass.enabled")
UiId::new("settings.reset-cache")
UiId::new("tutorial.next")
```

インデックスや描画順だけに依存したIDは避けてください。

## 4. コンポーネントはResponseを返す

描画するだけでなく、各コンポーネントが操作結果と配置矩形を返せる構造にしてください。

最低限、次のような情報を取得できる必要があります。

```rust
pub struct Response {
    pub id: UiId,
    pub rect: Rect,
    pub hovered: bool,
    pub pressed: bool,
    pub clicked: bool,
    pub focused: bool,
    pub changed: bool,
}
```

正確なフィールドは既存アーキテクチャに合わせて調整して構いません。

重要なのは、将来次のような処理ができることです。

```rust
let response = ui.button(...);

if response.clicked {
    // action
}
```

または、既存の`AppAction`方式を維持する場合は、コンポーネントが`HitTarget`やアクションを一貫して生成できる形でも構いません。

描画、ヒットテスト、アクションの対応関係が別々の巨大な`match`へ分散しない構造にしてください。

## 5. コンポーネントの矩形を後から参照可能にする

チュートリアル側から、特定の`UiId`に対応する画面上の矩形を取得できるようにしてください。

例:

```rust
ui.rect(UiId::new("settings.liquid-glass.enabled"))
```

または同等のRegistry APIを用意してください。

これにより、将来次を実装できる必要があります。

* 対象UIのスポットライト表示
* 吹き出しの位置決定
* 対象が画面外にある場合の自動スクロール
* 特定の入力以外を一時的にブロック
* チュートリアル進行条件の判定

今回、チュートリアル自体を実装する必要はありません。

ただし、後から大きく作り直さずに追加できる基盤にしてください。

# 今回実装するコンポーネント

最低限、以下を共通コンポーネントとして実装してください。

* `Button`
* `IconButton`、またはアイコン付きButton
* `Toggle`
* `Slider`
* `Label`
* `Heading`
* `Divider`
* `Row`
* `Column`
* `Spacer`
* `ScrollView`

必要であれば内部的な`Widget` traitやenumを導入して構いません。

ただし、将来の要件を予想して過剰に汎用化しないでください。今回実際に設定パネルで使う機能を中心に、拡張可能な最小構成としてください。

# ScrollViewの要件

Issue #122の主目的として、設定パネルのDebugカテゴリーをピクセル単位で滑らかにスクロールできるようにしてください。

## 入力

現在の`settings_scroll_rows: i32`を、論理ピクセル単位の`f32`オフセットへ変更してください。

例:

```rust
settings_scroll_y: f32
```

`MouseScrollDelta::PixelDelta`は丸めず、そのまま高精度デルタとして扱ってください。

`MouseScrollDelta::LineDelta`は、適切な論理ピクセル数へ変換してください。

`WindowEvent::MouseWheel`の`TouchPhase`も入力に含めてください。

macOS版winitでは、同じネイティブスクロールイベントから`DeviceEvent::MouseWheel`と`WindowEvent::MouseWheel`の両方が生成される可能性があります。

同じ入力を二重に適用しないようにし、原則として位相情報を持つ`WindowEvent::MouseWheel`を利用してください。

## スクロール物理

macOS／iOSに近い挙動を目標にしてください。

最低限:

* トラックパッドの細かい動きへ1:1で追従
* ピクセル単位の停止位置
* 慣性スクロール
* 自然な減速
* 上端・下端のラバーバンド
* 端からのばね復帰
* 60Hz／120Hz／144Hzで大きく感触が変わらないこと

既存の`src/scroll.rs`には、以下の実装があります。

* 慣性
* ばね
* ラバーバンド
* サブステップ
* フレームレート非依存の積分

これらを可能な限り再利用または一般化してください。

既存の水平ページング用`Scroller`を無理に流用して複雑にするのではなく、必要であれば次のように責務を分離してください。

```rust
PagingScroller
ContinuousScroller
```

または、共通の1次元物理コアと、ページング／連続スクロールのポリシーに分けてください。

macOSからOS側のモメンタムイベントが届く場合に、独自慣性を二重適用しないよう注意してください。

## クリップ

スクロール領域は、タイトルバーやパネル外へ描画が漏れないようにしてください。

各`TextView`や`InkView`へ毎回`Option<Rect>`を付けるだけの設計ではなく、可能であればコンテナまたはバッチ単位でクリップ領域を指定できる構造にしてください。

例:

```rust
ClipId
ClipRegion
```

```rust
GlyphBatch {
    clip: Option<ClipId>,
    ...
}
```

```rust
InkBatch {
    clip: Option<ClipId>,
    ...
}
```

矩形クリップについては、可能であればWGSL内のピクセル判定ではなく、`wgpu::RenderPass::set_scissor_rect()`を利用してください。

将来、複数のスクロール領域やモーダルで再利用できる構造にしてください。

## スクロールバー

macOS／iOS風のオーバーレイ式スクロールバーを実装してください。

要件:

* コンテンツ上に重なる
* スクロール開始時に表示
* スクロール中は表示を維持
* 停止後に少し待ってフェードアウト
* ホバー／ドラッグ中は太くなる
* サムをドラッグしてスクロール可能
* コンテンツ量に応じてサムの長さを計算
* オーバースクロール中は端側で縮む
* スクロール領域ごとに独立した状態を持つ

Appleの非公開実装値を推測して固定するのではなく、調整可能なテーマ値として実装してください。

例:

```rust
ScrollbarStyle {
    idle_width,
    active_width,
    minimum_thumb_length,
    hold_duration,
    fade_duration,
    inset,
}
```

# レイアウト

現在の設定パネルでは、各行のY座標を手動計算しています。

共通の`Column`、`Row`、`Spacer`などを使い、少なくとも設定パネル内部では画面側が行番号と座標を直接管理しなくてよい構造にしてください。

例:

```rust
ui.column(|ui| {
    ui.heading(...);
    ui.toggle(...);
    ui.slider(...);
    ui.button(...);
});
```

完全なFlexboxやCSSレイアウトエンジンを実装する必要はありません。

最低限、以下が扱えれば十分です。

* 縦方向の連続配置
* 横方向の配置
* 固定間隔
* 内側余白
* 幅いっぱいに広がる行
* 右寄せされた補助コントロール
* スクロールコンテナ内のコンテンツサイズ計算

# 状態管理

ウィジェット状態は、画面固有の値と、UI内部の一時状態を分離してください。

画面側の状態:

* トグルの値
* スライダーの値
* 設定値
* 永続化対象

UI内部の状態:

* hovered
* pressed
* focus
* スクロール速度
* スクロールバーの透明度
* 押下アニメーション
* ホバーアニメーション

安定した`UiId`をキーに、必要な一時状態を保存できる構造を検討してください。

ただし、今回必要のない巨大な仮想DOMや差分エンジンは導入しないでください。

# 設定パネルの移行

新しいUIコンポーネント基盤を作るだけでなく、PR #121で追加された設定パネルの主要UIを実際に新基盤へ移行してください。

最低限、次を移行対象にしてください。

* カテゴリー選択
* 通常の設定行
* Button
* Toggle
* Slider
* Reset icon
* Divider
* Debugカテゴリー
* ScrollView
* スクロールバー
* ヒットテスト

移行後、設定パネル側で次を直接組み立てるコードを大幅に減らしてください。

* `InkView`の個別push
* スライダートラック／ノブの個別生成
* トグルのトラック／ノブの個別生成
* 描画とは別に書かれた重複ヒットテスト
* 行番号ベースの表示判定
* 手動のスクロール座標補正

既存の見た目や操作を壊さない範囲で段階的に移行して構いません。

# 既存アーキテクチャとの整合

以下の責務分離は維持してください。

```text
UI component layer
    ↓
RenderModel + HitMap
    ↓
Renderer
    ↓
wgpu / WGSL
```

UIコンポーネントから直接`wgpu::Device`、`wgpu::Queue`、`wgpu::RenderPass`を操作しないでください。

スクロール物理やレイアウト計算も、可能な限り`wgpu`へ依存させないでください。

既存の`RenderModel`、`GlassSurface`、`InkView`、`TextView`、`GlyphView`、`HitMap`は、必要に応じて拡張・整理して構いません。

ただし、アプリ固有の設定値やLiquid Glassパラメーターを、汎用UI層へ直接依存させないでください。

# Liquid Glass

Button、Toggle、SliderなどがLiquid Glassスタイルを利用できるようにしてください。

ただし、全ての小さなコントロールを必ず個別の高コストなLiquid Glassパスで描画する必要はありません。

以下を考慮し、既存レンダラーと相性のよい方式を設計してください。

* 通常のInk描画
* Liquid GlassのGlassSurface
* Control用GlassBehavior
* 複数コントロールのバッチング
* 描画順
* 背景ブラー
* エッジライティング
* ホバー／押下アニメーション
* GPU負荷

見た目の指定と、レンダリング戦略を分離してください。

画面側は`ButtonStyle::Prominent`などを指定するだけでよく、内部でLiquid Glassを使うか、軽量なInk表現を使うかはテーマ／レンダラー側で決定できる構造が望ましいです。

# 将来のチュートリアル対応

今回チュートリアル画面は実装しません。

ただし、次を後から追加できることを設計上確認してください。

```rust
let target = ui_registry.rect(UiId::new("settings.liquid-glass.enabled"));
```

```rust
scroll_view.ensure_visible(
    UiId::new("settings.liquid-glass.thickness"),
    ScrollAlignment::Center,
);
```

以下の用途を想定してください。

* UI要素のスポットライト
* 吹き出し
* 次へ／戻る
* 特定操作完了時の自動進行
* 対象要素への自動スクロール
* 一部入力のブロック
* 画面サイズ変更後の対象追従

このため、UI要素のIDと最終配置矩形を取得できることは必須です。

# テスト

以下の単体テストを追加してください。

## UI基盤

* 同じ入力から同じレイアウトが生成される
* コンポーネントIDと矩形がRegistryへ登録される
* Buttonのヒットテスト
* Toggleのヒットテスト
* Sliderの値変換
* Sliderのクランプ
* コンポーネントの描画矩形とヒット矩形が一致する
* Column／Rowの配置
* DPI変更時のサイズ計算

## ScrollView

* ピクセル単位でオフセットが変化する
* 行単位デルタがピクセルへ変換される
* 上端／下端のクランプ
* ラバーバンドが入力量に対して非線形になる
* ばねで有効範囲へ戻る
* 慣性が時間とともに減衰する
* 60Hz／120Hzで最終結果が大きくずれない
* スクロールバーのサム位置
* スクロールバーの最小長
* フェードイン／フェードアウト
* スクロールバーのドラッグ
* `ensure_visible`
* クリップ領域外の要素が描画対象から漏れない、またはscissorが正しく設定される

## 回帰テスト

* 設定カテゴリー切り替え
* 既存トグル
* 既存スライダー
* 個別リセット
* Liquid Glass一括リセット
* ウィンドウ装飾
* アイコンキャッシュ再構築
* デバッグフラグ
* 設定永続化
* ヒットテスト
* 設定パネル開閉アニメーション

# 手動QA

WindowsとmacOSの両方で確認してください。

確認項目:

* マウスホイールで自然にスクロールできる
* トラックパッドで指の動きに追従する
* 行の途中で停止できる
* 高速スクロール後に自然に減速する
* 上端／下端でラバーバンドする
* 指を離すと自然に戻る
* スクロールバーが表示・フェードする
* スクロールバーをドラッグできる
* スクロール中もトグルやスライダーのヒット位置がずれない
* タイトルやパネル外へ描画が漏れない
* 100%／125%／150%／200% DPIで破綻しない
* 60Hz／120Hzで極端に挙動が変わらない
* macOSでスクロール量が二重適用されない

可能であれば`LAUNCHPAD_ALLOW_SCREENSHOT=1`を利用して視覚確認も行ってください。

# 非目標

今回は以下を実装しないでください。

* 公開crate化
* 別リポジトリ化
* HTML／CSS相当の完全なレイアウトエンジン
* 完全なアクセシビリティ基盤
* チュートリアル画面そのもの
* 全画面の一括移行
* 既存レンダラーの全面的な書き換え
* `egui`や`iced`へのアプリ全体の移行
* 巨大な仮想DOM
* 使用予定のない多数のウィジェット

# 実装の進め方

最初に現在の以下を確認し、既存構造を踏まえた実装計画を提示してください。

* `src/ui_model/`
* `src/layout/settings_panel.rs`
* `src/app/render/settings.rs`
* `src/renderer/prepare.rs`
* `src/renderer/frame.rs`
* `src/scroll.rs`
* `src/app/handler.rs`
* `HitMap`／`HitRegion`
* テキスト描画
* Liquid GlassのGlassSurface／GlassBehavior

その後、次の順序で進めてください。

1. UI基盤の最小APIを設計
2. `Response`、ID、Registry、レイアウトコンテナを実装
3. Button／Toggle／Sliderを実装
4. ScrollViewとクリップを実装
5. スクロールバーを実装
6. 既存スクロール物理を一般化
7. 設定パネルを新コンポーネントへ移行
8. 既存の重複コードを削除
9. テストを追加
10. Windows／macOSでQA

一度に大規模な置き換えをして動作不能にするのではなく、各段階でビルドとテストが通る状態を維持してください。

# 完了条件

以下を全て満たした時点で完了とします。

* 設定パネルが共通Button／Toggle／Slider／ScrollViewを利用している
* 画面側で描画プリミティブを個別に組み立てるコードが大幅に減っている
* 描画とヒットテストで座標計算が重複していない
* 各コンポーネントに安定したIDがある
* IDから最終配置矩形を取得できる
* Debugカテゴリーがピクセル単位で滑らかにスクロールする
* 慣性、ラバーバンド、ばね復帰がある
* macOS風のアニメーション付きスクロールバーがある
* スクロール領域が正しくクリップされる
* スクロール中もヒットテストが正しい
* Liquid Glassの見た目とウィジェットの挙動が分離されている
* 将来ほかの画面とチュートリアルで同じコンポーネントを利用できる
* `cargo fmt --check`が通る
* `cargo clippy --all-targets --all-features`が警告なしで通る
* `cargo test`が全て通る
* 既存の設定機能に回帰がない

実装完了後、以下を報告してください。

* 導入したUIアーキテクチャ
* 公開した主要API
* 設定パネルの変更点
* スクロール物理の設計
* クリップ方式
* スクロールバーの設計
* Liquid Glassとの分離方法
* 将来のチュートリアルでの利用方法
* 追加したテスト
* 手動QA結果
* 残っている制約や今後の改善候補

