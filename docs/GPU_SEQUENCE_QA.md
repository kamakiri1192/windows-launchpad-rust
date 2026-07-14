# GPU シナリオ連番 QA

## 目的

`computer-use`、前面ウィンドウ、画面キャプチャ API に依存せず、長押し・ドラッグ・フォルダ内ページスワイプを production の入力経路と GPU 描画経路で再生し、時系列で確認するための仕組みです。

シナリオモードではランチャーのウィンドウを非表示で作成し、描画済みの swap-chain texture を GPU から直接読み戻します。物理モニターや前面表示は不要です。Windows のグラフィカルセッションと利用可能な WGPU adapter は必要で、完全に GPU のないサーバーでは software adapter の構成が別途必要です。

## 実行方法

リポジトリにはフォルダ操作をまとめて確認するシナリオがあります。

```powershell
cargo build --release
$env:LAUNCHPAD_QA_SCENARIO = (Resolve-Path .\qa\folder_interactions.json).Path
.\target\release\launchpad-windows.exe
```

`qa/folder_creation.json` は、トップレベルのアプリを長押しして別アプリ上で保持し、Liquid Glass の融合previewから実際のフォルダ作成へ到達する経路を確認します。

シナリオの `duration_ms` に達するとプロセスは自動終了します。出力先はシナリオの `output_dir` 配下に実行時刻付きディレクトリとして作られます。

```text
target/qa-sequences/folder-interactions-<timestamp>/
├── frame_000000.png
├── frame_000001.png
├── ...
├── manifest.json
└── scenario-source.txt
```

既存ディレクトリを削除・上書きせず、同時実行した複数ブランチの結果を分離します。

## 連番と動画

`manifest.json` には各フレームの経過時間、編集モード、フォルダの開閉、ページ番号、リネーム状態に加え、フォルダページの scroll 位置・速度・physics phase、子アプリドラッグ、トップレベルドラッグ、トップレベル項目数、開いているフォルダの子数を記録します。さらに、実フレーム時間、pointer座標、入力から期待されるscroll位置と実位置の誤差、スナップ先、速度サンプル数、フレーム間scroll量、pointer move回数、再レイアウト回数、子アプリの端保持先と進捗を記録します。見た目のガクつきが入力追従・イベント間隔・release velocity・snap のどこで生じたか、フォルダ境界でドラッグの所有権とモデルが同じフレームに引き継がれたかを連番と数値で突き合わせられます。また、その実行結果を MP4 にする `ffmpeg` コマンドも格納します。

```powershell
cd target\qa-sequences\folder-interactions-<timestamp>
ffmpeg -framerate 30 -i frame_%06d.png -c:v libx264 -pix_fmt yuv420p folder-interactions.mp4
```

動画エンコーダーを必須依存にせず、CI では連番 PNG を画像差分や artifact として扱えます。

連番QAは各キャプチャでGPU readbackを行うため、絶対的なGPU負荷やフレーム時間のベンチマークには使用しません。状態遷移、入力追従誤差、相対的なフレーム変化の確認に限定し、CPU/GPUの外形監視とGPUパス別の性能分析は [GPU / CPU パフォーマンス計測ガイド](PROFILING_EVALUATION.md) に従います。

## シナリオ形式

シナリオ JSON は次の要素で構成します。

- `viewport`: GPU 出力の物理ピクセルサイズ。
- `fps`: 連番の取得頻度。1〜120に制限されます。
- `duration_ms`: シナリオ終了時刻。
- `fixture`: OS の Start Menu に依存しないアプリ、フォルダ、トップレベル順。
- `actions`: `at_ms` で指定する時刻付き操作。

主な操作:

- `open_folder`: 安定 ID でフォルダを開く。
- `move`: pointer をセマンティックな対象へ移動する。
- `pointer_down` / `pointer_up`: production と同じ press/release classifier を通す。
- `type_text` / `commit_rename`: フォルダ名入力を検証する。
- `escape` / `exit_edit_mode`: 状態の優先順位を検証する。

`move.target` は絶対座標のほか、`grid_item`、`folder_child`、`folder_title`、フォルダパネル内の相対位置を指定できます。レイアウト変更後も固定座標を書き直さずに同じ意図を再生できます。

## 長押しとスクロールの検証

長押しは `editing = true` を直接設定しません。`pointer_down` を保持したまま通常の `Tick` を500ms以上進め、production の長押し判定で編集モードへ入ります。フォルダページもページ番号を直接変更せず、pointer の移動量と release 速度を通常の `Scroller` へ渡します。

同梱シナリオは次を1本の連番で記録します。

1. 11個の子アプリを持つフォルダの開くアニメーション。
2. 子アプリ長押しによる編集モードと wiggle。
3. フォルダ内並べ替え。
4. 太字タイトルの名前編集、文字入力、確定。
5. 編集モード終了後の横スワイプと2ページ目へのスナップ。
6. フォルダを閉じるアニメーション。

追加シナリオ:

- `qa/folder_creation.json`: アプリ同士の Liquid Glass 融合プレビューから、2アプリのフォルダが作成されて開くまでを記録します。
- `qa/folder_single_page_scroll.json`: 最初の長押しで子アプリをそのまま持ち上げて並べ替えた後、1ページだけのフォルダの空き領域を横方向へ引っ張ります。`folder_child_drag` と `folder_scroll_phase` が同時に競合せず、`folder_scroll_x` がドラッグ中に0へ強制リセットされず、`Dragging` から `Settling` を経て `Idle` に戻ることを60fpsで確認します。
- `qa/folder_child_page_drag.json`: 子アプリを長押ししたまま右端で保持し、pointerを離さず2ページ目へ送り、そのページのセルへ配置するまでを記録します。端保持の進捗、ページ番号、子ドラッグの所有権が連続することを確認します。
- `qa/folder_child_exit.json`: 子アプリを長押ししたまま上端からパネル外へ出し、フォルダを閉じながら同じpointerのトップレベルドラッグへ引き継いで配置するまでを記録します。`folder_child_drag` から `top_level_drag` への切り替えと項目数・子数の変化を確認します。
- `qa/folder_existing_drop.json`: トップレベルのアプリを既存フォルダへ重ね、既存フォルダを並べ替えで逃がさずにスプリングオープンし、子として追加されるまでを記録します。
- `qa/folder_top_level_drag.json`: 閉じたフォルダを長押しして移動し、Liquid Glass面と小アイコンが消えず、共通中心を保つ一体のプレビューとして拡大・wiggle・追従することを確認します。
- `qa/grid_vertical_reorder.json`: トップレベルのアプリを別の行へ斜めに運び、横距離に引っ張られず対象行へ縦方向に25%入った時点でライブ並べ替えが成立することを確認します。

## 安全性

- QA fixture はメモリ上の `AppRegistry` と `LauncherState` にだけ入れます。視覚差分でアプリアイコンとフォルダ内プレビューを識別できるよう、fixture専用の決定的な単色アイコンを一時atlasへ生成します。
- シナリオモードでは設定、並べ替え、フォルダ名を SQLite へ保存しません。
- Start Menu watcher の結果はシナリオ実行中に取り込みません。
- Liquid Glass のデスクトップ取り込みは停止し、初期 backdrop texture を使うため、ホストのデスクトップ内容を artifact に含めません。

## 単発キャプチャとの使い分け

`LAUNCHPAD_QA_SHOT_FILE` は手動操作中の任意の1フレーム取得に残します。状態遷移、長押し、フリック、開閉モーションを確認する場合は `LAUNCHPAD_QA_SCENARIO` を使います。
