# launchpad-windows

macOS Launchpad 風の操作感と Liquid Glass 表現を、Rust / winit / wgpu で実装する
GPU アクセラレーション対応のネイティブアプリランチャーです。Windows を主対象に
開発しており、macOS 14 以降の Apple Silicon にも対応しています。

現在のバージョンは `0.1.0` です。初期のダミータイル MVP は完了しており、実際の
アプリ検出・起動、検索、並べ替え、フォルダ、設定、永続化まで実装されています。

## 現在実装されている機能

- `winit` + `wgpu` による透過・ボーダーレス描画
- デスクトップを背景に使う Liquid Glass 表現
  - Windows: Windows Graphics Capture
  - macOS: ScreenCaptureKit + IOSurface の GPU-to-GPU 転送
  - 背景取得に失敗した場合は静的なフォールバックを表示
- OS にインストールされたアプリと Steam アプリの検出
  - Windows: ユーザー / 全ユーザーのスタートメニューにある `.lnk`
  - macOS: 標準の Applications ディレクトリにある `.app`
  - Steam ライブラリの manifest からゲームとアプリを追加
- アイコン抽出のバックグラウンド処理と SQLite キャッシュ
- 起動中のアプリ追加・更新・削除を定期スキャンして差分反映
- 複数ページの水平スワイプ、慣性、スプリングスナップ、端のラバーバンド
- 下部コントロールの検索ピル / ページインジケーター / 検索フィールドへのモーフィング
- 大文字小文字を区別しない複数語検索と、日本語 IME の preedit / commit 対応
- アイコン長押しによる編集モード
  - ページ内・ページ間の並べ替え
  - 画面端でのページ自動送り
  - アプリの非表示
  - 並べ替え、非表示状態の永続化
- アプリ同士のドラッグによるフォルダ作成と、既存フォルダへの追加
  - フォルダ内ページングと並べ替え
  - フォルダからの取り出し
  - フォルダ名編集（IME 対応）
  - フォルダが 1 アプリ以下になった場合の自動解体
- 設定オーバーレイ
  - 名前順 / 手動などの並び順設定
  - Steam アプリの表示切り替え
  - 検索時に非表示アプリを含める設定
  - アイコンキャッシュと設定のリセット
- 常駐動作、単一インスタンス、グローバルショートカット、トレイ / メニューバー操作
- アイドル時の再描画停止と、表示中だけ動作する背景キャプチャ

## 対応環境

| OS | リリース対象 | 呼び出し | 常駐 UI |
| --- | --- | --- | --- |
| Windows | x86-64 (`x86_64-pc-windows-msvc`) | `Win+Space` | 通知領域のトレイアイコン |
| macOS | macOS 14+ / Apple Silicon (`aarch64-apple-darwin`) | `Option+Space` | メニューバーアイコン |

ソースからのビルドには、リポジトリで固定している Rust `1.89.0` が必要です。
Windows では MSVC ビルドツール、macOS では Xcode と macOS SDK を使用します。

macOS で実デスクトップを Liquid Glass の背景に使うには、初回起動時に
「システム設定 > プライバシーとセキュリティ > 画面収録とシステムオーディオ」から
許可してください。許可を変更した後はアプリを再起動します。

## 実行

滑らかな描画と実運用に近い挙動を確認するため、Release ビルドを推奨します。

```sh
cargo run --release --locked
```

ディスク上のアイコンキャッシュを削除してから起動する場合は、次を実行します。

```sh
cargo run --release --locked -- --reset-cache
```

GitHub Release では Windows x86-64 の ZIP と、macOS 14+ Apple Silicon 向けの
`Launchpad.app` ZIP を生成します。macOS 版は現時点では ad-hoc 署名で、Developer ID
署名と notarization は行っていません。

## 基本操作

ランチャーのウィンドウを閉じても、通常はプロセスを終了せず非表示になります。
グローバルショートカットやトレイ / メニューバーからすぐに再表示できます。

| 操作 | 動作 |
| --- | --- |
| `Win+Space`（Windows） | どこからでもランチャーを表示 |
| `Option+Space`（macOS） | どこからでもランチャーを表示 |
| 左ドラッグ | ページをスワイプ |
| アプリをクリック | アプリを起動してランチャーを非表示 |
| フォルダをクリック | フォルダを開く |
| 検索ピルをクリック | 検索フィールドを開く |
| アイコンを約 500 ms 長押し | 編集モードに入り、そのアイコンを持ち上げる |
| 編集中にアイコンをドラッグ | 並べ替え。別のアイコン / フォルダに重ねて待ってからドロップすると作成 / 追加 |
| 編集中に `×` バッジをクリック | アプリをランチャーから非表示 |
| 編集中に `完了` | 変更を保存して編集モードを終了 |
| 編集中に歯車ボタン | 設定を開く |
| `Esc` | 現在の状態に応じて名前編集、設定、編集モード、検索、フォルダを閉じ、何も開いていなければランチャーを非表示 |
| 他のウィンドウをクリック / Alt-Tab | 通常時はランチャーを自動で非表示 |

Windows のトレイアイコンでは「表示」「設定」「終了」を選択できます。macOS の
メニューバーにも同等の Show / Settings / Quit メニューがあります。プロセスを完全に
終了する場合は、各メニューの終了項目を使用します。

macOS のグローバルショートカットは `LAUNCHPAD_HOTKEY` で変更できます。

```sh
LAUNCHPAD_HOTKEY='shift+alt+Space' cargo run --release --locked
```

Liquid Glass の調整キーを含む全操作は
[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md) を参照してください。

## データとキャッシュ

アイコン、設定、並び順、非表示アプリ、フォルダ構成は 1 つの SQLite データベースに
保存します。

| OS | 保存先 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Launchpad\cache.sqlite3` |
| macOS | `~/Library/Application Support/Launchpad/cache.sqlite3` |

`LAUNCHPAD_DEBUG=1` を設定すると、Windows では
`%LOCALAPPDATA%\Launchpad\debug.log`、macOS では
`~/Library/Logs/Launchpad/debug.log` に診断ログを出力します。

## 開発と検証

標準の確認コマンドは次のとおりです。

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

Liquid Glass シェーダだけを調整する場合は、独立した開発用シミュレーターを利用できます。

```sh
cargo run --bin liquid_glass_studio --locked
```

詳細は [docs/LIQUID_GLASS_STUDIO.md](docs/LIQUID_GLASS_STUDIO.md) を参照してください。

### スクリーンショットを使う Visual QA

Windows の通常起動では、Liquid Glass の背景キャプチャとの再帰を避けるため
ランチャー自身を画面キャプチャ対象から除外しています。スクリーンショットを使う QA
では、起動前に `LAUNCHPAD_ALLOW_SCREENSHOT=1` を設定してください。

```powershell
$env:LAUNCHPAD_ALLOW_SCREENSHOT = '1'
$env:LAUNCHPAD_DEBUG = '1'
cargo run --release --locked
```

手順と注意点は [docs/EDIT_MODE_VISUAL_QA.md](docs/EDIT_MODE_VISUAL_QA.md)、連番 PNG を
使う決定的シナリオ QA は [docs/GPU_SEQUENCE_QA.md](docs/GPU_SEQUENCE_QA.md) にあります。

## プロジェクト構成

```text
src/
├── main.rs          # プロセス起動、キャッシュ、worker、イベントループの接続
├── app/             # winit イベントの正規化、状態遷移、コマンド実行、描画の調停
├── domain/          # AppId、アプリ台帳、ランチャー状態、フォルダ、設定
├── features/        # 検索下部 UI、編集モード、フォルダなどの機能ロジック
├── layout/          # グリッド、フォルダ、設定、コントロールの配置と hit-test
├── ui_model/        # renderer 非依存の描画モデルと UI ID
├── renderer/        # wgpu リソース、パイプライン、描画パス
├── liquid_glass/    # 背景取得、Glass geometry、blur、合成
├── platform/        # Windows / macOS 固有の常駐 UI、hotkey、アプリ起動
├── workers/         # アプリ走査、Steam 走査、アイコン抽出、差分監視
└── bin/
    └── liquid_glass_studio.rs

assets/              # アプリアイコン、macOS bundle 情報、共有 WGSL
docs/                # 設計、実装ノート、QA、トラブルシューティング
qa/                  # 決定的 GPU シーケンス QA のシナリオ
tests/               # アーキテクチャ境界、domain 統合、WGSL 検証
```

目標とする依存方向、責務分離、イベントから描画までの流れは
[ARCHITECTURE.md](ARCHITECTURE.md) にまとめています。

## 現在の制約

- アプリ選択のキーボードナビゲーションと、検索結果を `Enter` で起動する操作は未実装です。
- 「最近使用」「よく使用」の設定値は保存されますが、使用履歴の収集と履歴ベースの
  並べ替え / ホーム表示は未実装です。
- 非表示アプリ数の表示はありますが、個別に再表示する管理 UI は未実装です。設定の
  リセットで非表示状態をまとめて解除できます。
- macOS の配布物は Apple Silicon / macOS 14+ のみで、Developer ID 署名と
  notarization はまだ行っていません。

## 関連ドキュメント

- [docs/FOLDER_INTERACTION.md](docs/FOLDER_INTERACTION.md) — フォルダ作成、編集、ページングの操作仕様
- [docs/BOTTOM_CONTROL.md](docs/BOTTOM_CONTROL.md) — 下部モーフィングコントロールの状態遷移
- [docs/STARTUP_PERFORMANCE.md](docs/STARTUP_PERFORMANCE.md) — 初回描画とバックグラウンド処理
- [docs/ICON_CACHE.md](docs/ICON_CACHE.md) — SQLite アイコンキャッシュ
- [docs/APP_REFRESH.md](docs/APP_REFRESH.md) — 起動中のアプリ差分検出
- [docs/MACOS_DEVELOPMENT.md](docs/MACOS_DEVELOPMENT.md) — macOS の権限、性能計測、配布
- [docs/PROFILING_EVALUATION.md](docs/PROFILING_EVALUATION.md) — CPU / GPU プロファイリング
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) — 既知の問題と調査記録
