# Phase 8 Folder Feature — Goal Prompt

> この文書は、Phase 7 完了後の `launchpad-windows` にフォルダ機能の縦スライスを実装するための、別セッション向け引き継ぎプロンプトです。

## 依頼

`docs/DF_REARCHITECTURE_PLAN.md` の **Phase 8: Folder Feature Vertical Slice** を、設計だけで終わらせず、実際に操作・永続化・画面確認できる状態まで実装してください。

自律的に調査、実装、テスト、画面 QA、ドキュメント更新まで進めてください。途中でコンパイルが通るだけの未配線な抽象化や、フォルダを描画するだけのデモ状態を完成とはしないでください。既存のユーザー向け挙動を維持し、フォルダ機能を Feature / Domain / Layout / Render Model / App Shell の境界を通る完全な縦スライスとして仕上げてください。

## 最初に確認する資料とコード

作業開始時に、少なくとも次を読んで現状を確認してください。

- `AGENTS.md`
- `ARCHITECTURE.md`
- `docs/DF_REARCHITECTURE_PLAN.md`
- `docs/DF_REARCHITECTURE_LOG.md` の Phase 6.5〜7
- `docs/DF_CURRENT_BEHAVIOR_INVENTORY.md`
- `docs/EDIT_MODE_VISUAL_QA.md`
- `src/domain/launcher_item.rs`
- `src/domain/folders.rs`
- `src/domain/launcher_state.rs`
- `src/features/edit_mode/`
- `src/layout/grid.rs`, `src/layout/edit_mode.rs`, `src/layout/hit_map.rs`
- `src/ui_model/`
- `src/app/action.rs`, `src/app/state.rs`, `src/app/update.rs`, `src/app/command.rs`
- `src/app/render/`
- `src/renderer/prepare.rs`
- `tests/launcher_domain_integration.rs`
- `tests/architecture_boundaries.rs`

作業ツリーに既存変更がある場合は、それをユーザーの変更として保持し、無関係な差分を巻き戻さないでください。

## 現在の基準状態

Phase 7 までで、次の基盤は実装済みです。この責務分離を後退させないでください。

- `AppRegistry` は Windows から再発見できるアプリ情報だけを所有する。
- `LauncherState` はトップレベルの `LauncherItem` 順、フォルダ、非表示アプリ、カスタマイズ状態を所有する。
- `LauncherItem::{App, Folder}`、`FolderId`、`Folder { name, children }` は serde 対応済み。
- `"launcher_state"` がユーザー配置の正規永続化形式である。
- 未発見アプリの ID は配置・フォルダ内にプレースホルダーとして保持し、再発見時に元の位置へ戻す。
- renderer は `LauncherItem`、`Folder`、`FolderId`、`LauncherState` を受け取らない。
- renderer の公開境界は renderer-neutral な `RenderModel` である。
- 現在のグリッド表示・クリック・編集モードは app-only の投影が残っており、Phase 8 で item-based に完成させる。

実装前に現行テストを一度実行し、ベースラインの失敗があれば記録してください。

## 最優先のビジュアル目標: iOS 26 Home Screen Folder + Liquid Glass

この Phase 8 では、フォルダ機能が「動く」だけでは不十分です。見た目と触感は **iOS のホーム画面でフォルダを開閉・作成・編集したときの空間的な連続性**を目標にしてください。特に iOS 26 の Liquid Glass に寄せ、Windows 向けの本アプリとして自然に翻案します。完全なピクセルコピーではなく、次の性質を再現することが受け入れ条件です。

参照する公式資料:

- Apple: [iOS 26 の新しいデザイン](https://www.apple.com/jp/os/ios/)
- Apple Human Interface Guidelines: [Materials / Liquid Glass](https://developer.apple.com/design/human-interface-guidelines/materials)
- Apple Developer: [Liquid Glass overview](https://developer.apple.com/documentation/TechnologyOverviews/liquid-glass)

### Liquid Glass は必須であり、装飾的な stretch goal ではない

- フォルダタイルと開いたフォルダパネルは、既存の production Liquid Glass pipeline を通して描画する。単なる半透明 RGBA 塗り、静的 gradient、擬似 blur への置き換えは禁止する。
- フォルダパネルは `GlassLayer::Modal` 上の dynamic `GlassSurface` とし、背後の壁紙・ホームグリッドを実際に blur、反射・屈折、彩度・光量調整、edge highlight、必要に応じた色収差の対象にする。
- 文字と多数の child icon を載せるため、パネルは原則として legibility の高い `GlassMaterial::Regular` 相当を使う。背景が十分に暗く、可読性を自動テスト・画面 QA で保証できる場合だけ、より clear な表現を検討する。
- ガラスの上に不透明な大面積 panel fill を重ねて Liquid Glass を隠さない。dimming layer は背後の content を沈めるための別 primitive とし、glass 自体とは分離する。
- 開閉時は glass の alpha だけを変えない。source folder tile の rect / corner radius / material presence から、最終 panel の rect / corner radius へ SDF shape を連続的に morph させ、背景の屈折領域も形状と一緒に変化させる。
- 既存の `liquid_glass` が持つ blur pyramid、IOR、chromatic aberration、tint、lighting、smooth-min merge、motion stretch を再利用する。folder 専用 shader を増やすのではなく、renderer-neutral な presentation data で既存 pipeline を駆動する。
- 既存 pipeline で動的な modal shape、複数 surface、morph 中の refraction が不足する場合は、汎用 Liquid Glass 能力として拡張する。renderer に `FolderId` や folder open state を渡して特別扱いしない。
- `docs/LIQUID_GLASS_STUDIO.md` と `liquid_glass_studio` を使い、panel の blur、tint、IOR、edge light、merge distance、spring、motion stretch を launcher 本体へ入れる前後で調整する。

### iOS ホーム画面らしいフォルダの見た目

- 閉じたフォルダタイルは既存 app tile と同じグリッド寸法・角丸リズムに合わせつつ、内部に child の先頭最大 9 個を **3×3 の miniature icon grid** として表示する。
- miniature icon は等間隔で、元 icon の縦横比を保ち、フォルダ tile の角丸からはみ出さない。発見済み child だけを描画するが、未発見 child を詰めて永続順序を変えてはならない。
- フォルダ名は通常の app label と baseline、font weight、最大幅を合わせる。長い名前は文字境界を壊さず省略する。
- 開いたパネル内は iOS folder の密度を目安に、1 page あたり最大 3 列×3 行とする。9 個を超える場合は横方向の folder-internal page と小さな page indicator を提供する。
- child が 9 個以下の場合、パネルは child 数に応じて自然に縮む。ただし cell size と gap は固定リズムを保ち、1〜2 個のときに巨大な空白や不自然な引き伸ばしを作らない。
- folder title はパネル上部に置き、通常表示から rename editor へその場で遷移する。タイトルやラベルは glass 上で十分なコントラストを保ち、明るい壁紙・暗い壁紙の両方で確認する。

### iOS ホーム画面らしい open / close motion

- pointer press 中は folder tile がごく小さく沈む press feedback を返し、release 後に source tile を起点として panel が開く。press feedback は操作を重く感じさせない 70〜100ms 程度を目安にする。
- opening は folder tile の glass shape が画面中央付近の panel へ拡大・移動する **container transform / zoom transition** とする。別の panel を中央で突然 fade-in させない。
- 背後の home grid は、opening と同期してわずかに拡大または後退する scale、dimming、soft blur を受け、選択した folder が前景へ浮く奥行きを作る。目安は scale 0.94〜0.97 または同等の視覚量、dimming 20〜35% だが、実画面の可読性を優先して調整する。
- miniature icons は source tile 内の 3×3 位置から、開いた panel の child cell 位置へ連続的に拡大・移動する。全 child を別位置で一斉 fade-in して空間的連続性を失わせない。2 page 目以降の child は panel が十分に開いた後で控えめに現してよい。
- folder title と page indicator は container motion よりわずかに遅れて materialize し、panel が閉じるときは先に dematerialize する。過剰な stagger や派手な bounce は避ける。
- opening は 360〜480ms、closing は 300〜420ms 程度を初期値とし、既存の iOS-style easing または軽く減衰した spring で QA 調整する。固定 60fps step ではなく `dt` ベースとする。
- spring は柔らかいが制御された動きにし、overshoot は視認できても 1〜3% 程度に抑える。panel の端が何度も跳ねる、icon がゴムのように遅れる、最後に pixel snap する挙動は禁止する。
- closing は現在の folder tile 位置へ戻る opening の厳密な逆遷移とする。トップレベル reorder、page scroll、resize 後も `FolderId` から最新 target rect を解決し、古い index へ戻さない。
- open/close の途中に反対操作が来たら、現在の presentation progress と velocity から滑らかに reverse する。終了待ちで input を固めたり、先頭/末尾へ snap して再開したりしない。
- animation 中に resize / DPI change / folder removal / app refresh が起きても panic や NaN を出さず、最新 layout へ retarget または安全に close する。

### drag と folder formation の motion

- app を別 app の上へ drag したときは、target tile がわずかに膨らみ、両方の Liquid Glass halo が smooth-min で引き合うように merge し、folder 化できることを視覚的に示す。
- hover threshold へ近づくにつれて target の scale / glass merge / miniature preview を連続的に進める。threshold 到達時に突然 folder tile へ置換しない。
- hover が確定したら iOS の spring-loaded folder のように preview を開き、drop で domain mutation を commit する。threshold 前に pointer が外れた場合や drag をキャンセルした場合は、元の app tile へ完全に戻し、空 folder を永続化しない。
- drop commit では dragged app が縮小しながら miniature grid または open panel の child cell へ吸い込まれ、残りの top-level items は既存 spring で隙間を詰める。
- 既存 folder への drag は folder tile の controlled scale-up、glass の粘性を感じる merge、miniature icon の reflow を見せる。folder が spring-open した後は dragged icon を pointer に追従させたまま child drop target を表示する。
- child reorder は既存 edit-mode と同様に周囲の child が spring で場所を空ける。drag-out は吸い込みの逆方向として panel edge を越えた時点から top-level cell へ滑らかに retarget する。
- animation は domain state と presentation state を混同しない。drop 前は preview、drop 後だけ永続 state とし、キャンセル可能性を保つ。

### presentation model と品質条件

- feature state には少なくとも phase、stable source/target identity、normalized progress、velocity、pending hover target、drag origin を持たせる。画面上の index だけで animation を追跡しない。
- layout は closed tile rect と open panel rect の両 endpoint、および現在の interpolated geometry を同じ source of truth から生成する。
- 背景 layer の scale / dim / blur が現行 `RenderModel` で表現できない場合は、folder 専用 renderer command ではなく汎用の layer presentation primitive として設計する。
- hit region は presentation geometry に追従させる。opening 中の見た目と closed/open のどちらか一方の古い hit rect がずれたまま残らない。
- endpoint、monotonic progress、reverse、retarget、frame-rate independence を deterministic test で検証する。60Hz、120Hz、animation frame drop 相当の大きな `dt` で終点が一致することを確認する。
- 「iOS 風」は主観だけで合格させない。source tile から panel への位置連続性、corner radius、background treatment、child trajectory、reverse symmetry、settling time を動画または連続 frame で確認する。

## 必須のユーザー体験

### 1. トップレベルのフォルダタイル

- 検索クエリが空の通常グリッドでは、`LauncherState.items` の `App` と `Folder` を同じ順序のトップレベル項目として表示する。
- ページ数、セル数、ヒットテスト、スクロール境界、並べ替えは「表示アプリ数」ではなく「表示項目数」を基準にする。
- フォルダタイルには、フォルダ名と、発見済み child の先頭最大 9 個を使った 3×3 の miniature icon grid を表示する。child が未発見の場合は ID と順序を保持し、描画だけを省略または placeholder にする。
- フォルダタイルの見た目も既存の `TileView` / `IconView` / `TextView` などの renderer-neutral primitive で表現する。renderer に folder 専用 DTO、setter、分岐を追加しない。
- 通常モードでアプリタイルをクリックした場合は従来どおり安定した `AppId` から起動し、フォルダタイルをクリックした場合だけフォルダを開く。

### 2. フォルダの open / close

- `features/folders/` に open/close、rename、folder 内 drag の feature state と純粋な遷移・intent を置く。
- 開いているフォルダは同時に 1 個までとする。状態は index ではなく `FolderId` で保持する。
- フォルダパネル外のクリックはフォルダを閉じ、そのクリックを背後のアプリへ replay しない。
- `Esc` は rename 中なら rename をキャンセルし、rename 中でなければフォルダを閉じる。もう一度 `Esc` を押したときだけ従来の launcher hide に進む。
- フォルダを開いている間は、通常グリッドの launch、透明領域 passthrough、横スクロールが背後で誤発火しないようモーダル入力 precedence を設ける。
- フォルダ内の通常クリックは child の安定した `AppId` を解決して起動する。未発見 child は起動不可とする。
- フォルダの open/close には、上記の container morph、background treatment、child icon trajectory を駆動する時間ベースの animation state を持たせ、安定した `UiId` で継続的に再描画する。reduce-motion 設定は現在存在しないため新設不要だが、frame-rate 依存の固定 step にはしない。

### 3. 動的フォルダパネル layout

- `layout/folder_panel.rs` を追加し、viewport、scale factor、フォルダ名、表示可能 child、animation progress などの明示的 input から `LayoutResult` を構築する。
- パネルの Liquid Glass は child 数と viewport に応じて動的にサイズ決定し、画面外へはみ出さない。空・1 個・多数、最終行が不完全な場合も破綻させず、9 個を超える場合は folder-internal pagination を使う。
- パネル、タイトル、child tile/icon/label、rename editor、modal backdrop、drop region の描画矩形と hit region は同じ計算結果から作る。
- パネルは dynamic `GlassSurface`、タイトルと child label は `TextView`、child は既存の tile/icon primitive を使う。
- 必要なら `UiId`、`HitTarget`、汎用的な overlay/glyph lane を拡張してよい。ただし renderer が feature 名や `UiId` の文字列を見て GPU pass を選ぶ設計にはしない。
- DPI 100% / 150% などで物理座標と論理座標を混同しない。

### 4. rename と IME

- 開いたフォルダのタイトルから rename を開始できる明確な hit target を設ける。
- rename は UTF-8 を正しく扱い、日本語 IME の preedit / commit、Backspace、左右移動、Enter 確定、Esc キャンセルを扱う。
- 既存検索欄の text/IME 実装を再利用可能な形で参照し、検索欄と rename editor が同時に keyboard focus を持たないようにする。
- 空文字または空白だけで確定した場合は、決定的な既定名 `フォルダ` に正規化する。不要な文字数制限は設けず、長い名前は layout 側で安全にクリップまたは省略表示する。
- rename 確定時だけ `LauncherState` を更新して永続化する。`Esc` キャンセルでは永続 state を変更しない。

### 5. drag-to-create / drag-into-folder / child ordering

- 編集モードでトップレベルのアプリを別のトップレベルアプリ上に一定時間 hover すると Liquid Glass merge と spring-loaded preview を開始し、そこで drop すると新しいフォルダを作る。
- 新規フォルダは target app がいた位置を引き継ぎ、child 順は `[target_app, dragged_app]` とする。dragged app の元セルは閉じる。
- `FolderId` は `LauncherState::next_folder_counter()` を使って衝突なく生成し、既定名は `フォルダ` とする。
- トップレベルのアプリを既存フォルダ上へ drop すると、そのアプリをトップレベルから除き、フォルダ child の末尾へ追加する。
- フォルダパネル内では child の live reorder と drop 確定を実装し、順序を永続化する。
- child をパネル外の有効なトップレベルセルへ drag-out できるようにし、その位置へ `LauncherItem::App` として戻す。
- child を別フォルダへ drop した場合はフォルダ間移動として扱い、同一アプリが複数箇所へ存在しないことを保証する。
- フォルダから child が減って 1 個になった場合はフォルダを解体し、残った child を元のフォルダ位置へ昇格する。0 個ならフォルダだけを削除する。開いていたフォルダが解体された場合は安全に close する。
- drag target は見た目の矩形と同じ `HitMap` region から決定する。単なる pointer 座標の重複計算や magic index による判定を増やさない。
- hover-to-create / hover-to-open は瞬間発火させず、時間閾値と現在の安定した item ID を state に持つ。pointer が target から外れたら pending hover を解除する。
- drop が無効、対象が自分自身、既に child、未発見 item などのケースでは state を壊さない。

### 6. 検索・編集モード・永続化との整合

- 検索クエリが空でない間は、従来互換として発見済みアプリの flat な検索結果を表示し、フォルダタイルは検索結果に出さない。検索中は folder create / reorder を無効にする。
- hidden app はトップレベルにも folder child にも現れないという Phase 7 invariant を維持する。
- discovery refresh でアプリが消えても、トップレベル位置、folder membership、child order を失わない。再発見時に同じ位置へ復帰する。
- 変更操作は統一された `launcher_state` JSON を永続化する。旧 `app_order` / `hidden_ids` を復活させたり、別の folders-only key を新設したりしない。
- フォルダを含むトップレベル reorder で app と folder の interleave を保つ。Phase 7 の「apps を並べて folders を末尾へ送る」暫定ロジックを Phase 8 の production path に残さない。
- app-only 前提の `visible_app_ids()`、`grid_apps_owned()`、spring key、drag state、click resolution を監査し、必要な箇所を stable `LauncherItem` identity ベースへ一般化する。

## レイヤー境界

- `domain/`: 永続データと純粋な配置 invariant。winit、wgpu、Win32、frame timing を入れない。
- `features/folders/`: folder UI の状態遷移、intent、純粋な mutation plan / command production。GPU や DB を直接触らない。
- `layout/folder_panel.rs`: geometry、primitive、hit region。app registry や副作用を所有しない。
- `app/`: raw event の正規化、feature dispatch、domain mutation、command 実行、frame tick の統合。
- `renderer/`: `RenderModel` の汎用 primitive を描画するだけ。`Folder` 等の domain 型を import せず、folder open/rename/create を知らない。
- `main.rs`: process startup のままとし、folder match arm や feature state を追加しない。

既存 primitive で不足する表現があれば、まず renderer-neutral な汎用 primitive として追加できるか検討してください。shader / `#[repr(C)]` / bind group layout を変更する場合は、その必要性を説明し、Rust と WGSL の layout および validation test を同時に更新してください。

## 必須テスト

少なくとも次を deterministic test で追加してください。

- folder open/close/rename の state transition と Esc/Enter precedence
- folder open/close animation の endpoint、reverse、retarget、60/120Hz 相当の frame-rate independence
- source folder tile → panel → source tile の geometry continuity と child trajectory
- Liquid Glass modal surface、background dim/blur presentation、glass material/layer の出力
- folder panel の動的サイズ、viewport clamp、DPI、空/1/4/5/多数 child
- 1 page 9 child、10 child 以上の folder-internal pagination と page indicator
- panel/title/child/backdrop/drop region の z-order と hit precedence
- drag-to-create の item 位置、child 順、stable ID 生成
- hover threshold 前の cancel、Liquid Glass merge preview、drop 前に domain state を変更しないこと
- drag-into-folder、child reorder、drag-out、folder 間移動
- 1 child / 0 child になった folder の解体 policy
- app と folder が interleave したトップレベル reorder
- duplicate/self-drop/invalid target/no-op の安全性
- serde round-trip と再起動相当の persistence
- app removal / rediscovery 後の folder membership と child order
- folder 内 child の安定 ID launch resolution
- 検索中の flat app 結果と folder edit 無効化
- renderer が folder domain concepts を import/receive しない architecture boundary

既存テストの期待値を都合よく弱めないでください。仕様変更で更新が必要な場合は、なぜ変更したかをログへ記録してください。

## 画面 QA

UI 変更のため、`docs/EDIT_MODE_VISUAL_QA.md` の手順に従って release build で画面確認してください。通常は以下を設定します。

```powershell
$env:LAUNCHPAD_ALLOW_SCREENSHOT = '1'
$env:LAUNCHPAD_DEBUG = '1'
```

実データを壊さないよう、一時 `LOCALAPPDATA` を使ってください。必要なら `LAUNCHPAD_QA_SHOT_FILE` の GPU self-capture を使い、少なくとも次を確認します。

- 既存の launcher 初期画面が non-blank である
- app と folder が混在したグリッド、folder 名、3×3 miniature icons
- source tile から panel へ連続する iOS-style open / close animation
- opening 中の Liquid Glass shape morph、背景の実 blur / refraction、edge light、dimming
- background grid の scale/blur/dim と、child icon が miniature position から展開する軌道
- animation 途中での reverse、resize、DPI change、reorder 後の close target
- child 数による panel resize、最終行、長い folder 名
- 9 child と 10 child 以上の internal page、page indicator、横 page 操作
- rename の直接入力、日本語 IME preedit / commit、Enter / Esc
- drag-to-create、drag-into-folder、child reorder、drag-out、folder 間移動
- folder 解体後のトップレベル配置
- restart 後も folder 名、item 順、child 順が一致する
- resize / DPI、horizontal scroll / inertia / snap / rubber-band
- app launch、検索、edit mode、Done、hide badge、settings gear
- folder open 中に背後の app launch / scroll / click passthrough が誤発火しない
- 通常時の透明領域 click passthrough は従来どおり動く

自動操作の制約で確認できない項目は、確認済みと書かず、制約と未確認範囲を明記してください。画面を確認できない場合は Phase 8 完了を主張しないでください。

静止画だけでは motion quality を検証できません。可能なら 60fps 以上の画面録画を残してください。録画が使えない場合は GPU self-capture の連続 frame、または progress 0 / 0.25 / 0.5 / 0.75 / 1.0 の deterministic capture を作り、tile→panel の軌道、glass shape、背景処理、child trajectory、closing の逆対称性を比較してください。

## 完了前の検証

以下を実行し、全ての結果を最終報告に含めてください。

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features
cargo build --release
cargo run --release
```

さらに差分を自分でレビューし、次を確認してください。

- domain invariant を壊す経路がない
- index ではなく stable ID で press/drag/animation を追跡している
- renderer に folder-specific concepts が漏れていない
- folder panel が production Liquid Glass pipeline を通り、半透明の代用品になっていない
- open/close が source tile と空間的に連続し、closing が最新 tile rect へ戻る
- drag hover / cancel / drop の presentation state と永続 domain state が分離されている
- modal input precedence と click passthrough が競合しない
- rename focus と search focus、IME enable/disable が競合しない
- animation 中だけ redraw が継続し、idle 時に busy loop しない
- persistence failure が UI state を panic させない
- 変更していない既存機能を退行させていない

## ドキュメント更新

- `docs/DF_REARCHITECTURE_LOG.md` に Phase 8 の実装内容、責務境界、domain policy、変更ファイル、テスト数、コマンド結果、画面 QA、未確認項目を追記する。
- 実装中に判明した現行挙動や pointer precedence がある場合は `docs/DF_CURRENT_BEHAVIOR_INVENTORY.md` を更新する。
- folder 操作の画面 QA に追加手順が必要なら `docs/EDIT_MODE_VISUAL_QA.md` を更新する。

## 完了条件

次の全てを満たしたときだけ完了です。

- フォルダが production path のトップレベル項目として表示・操作できる。
- open/close、rename、create、into、child reorder、out、between、auto-dissolve が動作する。
- folder tile、panel、drag merge が production Liquid Glass で描画され、iOS-style container morph と background treatment を備える。
- motion が frame-rate independent、interruptible、reversible で、source/target への snap や不自然な fade-only transition がない。
- 全変更が統一 `launcher_state` に永続化され、restart と discovery refresh に耐える。
- layout が描画と hit region の単一の source of truth になっている。
- renderer は folder semantics を知らない。
- 必須テスト、fmt、clippy、release build が通る。
- Screen Verification Gate を実施し、結果を正直に記録している。
- 一時実装、未配線コード、無効化したテスト、説明のない TODO を残していない。

最終報告は、最初に実装結果を短く述べ、その後に主な設計判断、変更ファイル、検証コマンド結果、画面 QA チェックリスト、残課題または未確認項目をまとめてください。
