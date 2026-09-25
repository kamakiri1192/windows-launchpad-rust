# 未分類アプリの確認と追加ルール

2026-09-25、このWindows端末の152項目を読取専用スキャンで確認した。
ユーザーの「Toolbox等もフォルダに含める」という指示に合わせ、管理アプリと製品付属ツールも明示的に対象へ追加した。AI分類は使用しない。

## 今回追加・拡張した配置

| フォルダ | この端末での対象 | 項目数 |
| --- | --- | ---: |
| JetBrains | IntelliJ IDEA、Toolbox | 2 |
| Adobe Creative Cloud | 制作アプリ10項目とCreative Cloud管理アプリ | 11 |
| Git（追加） | Git Bash、Git CMD、Git GUI | 3 |
| Unity（追加） | Editorの3版、Hub、各版のBug Reporter | 7 |
| Live2D Cubism（追加） | Editor 5、Viewer 5 | 2 |
| Maxon | Cinema 4D、Commandline、Team Render Client/Serverの2025/2026版 | 8 |
| Razer（追加） | Cortex、Synapse | 2 |
| アクセシビリティ（追加） | スクリーンキーボード、ナレーター、拡大鏡、ライブキャプション、音声アクセス | 5 |
| Windowsツール | 既存の11項目に管理ツール、Application Verifier、PowerShell/ISE、コマンドプロンプト、ステップ記録、タスクマネージャー、文字コード表を追加 | 22 |

Steamライブラリ4項目、Visual Studioツール6項目は引き続きそれぞれにまとめる。
全体は **121単体アプリ＋4フォルダ＝125項目** から **80単体アプリ＋11フォルダ＝91項目** になる。
152個の起動項目そのものは削除せず、配置だけを変更する。

新設5候補はこの端末で確認したWindowsの実行ファイル・配置・引数を使う。
JetBrains ToolboxはWindowsのProductName/CompanyNameとmacOSの既存確認済みbundle IDに対応する。

### 判定条件

- Git: `Git`配下の `git-bash.exe` / `git-cmd.exe`（空引数または `--cd-to-home`）、`git-gui.exe`（空引数）。
- Unity: `Unity`配下の `Unity.exe` / `UnityBugReporter.exe`、`Unity Hub`配下の `Unity Hub.exe`。いずれも空引数。同じEditorの複数版だけなら平置きを維持する。
- Live2D: `Live2D Cubism <数値版>` 配下の `CubismEditor5.exe` / `CubismViewer5.exe`。空引数。
- Maxon: `Maxon Cinema 4D <数値版>` 配下の4種類の限定exe。版違いは同じ製品・付属機能として数える。
- Razer: `Razer`配下の `RazerCortex.exe` と、`RazerAppEngine.exe --url-params=apps=synapse`。App Engineの別起動先を一括で含めない。
- アクセシビリティ: SystemRoot/System32/SysWOW64の限定exeとMicrosoftのCompanyName。空引数。
- Windowsツール: OSの既定実行パスとMicrosoftのCompanyName。共有 `control.exe` は `/name Microsoft.AdministrativeTools` に限定。Task Managerは空引数または実機で確認した `/7`。開発者PowerShellはVisual Studio側のルールを使う。

### 製品情報の読取り修正

IntelliJとToolboxのProductName/CompanyNameがアプリ内スキャンでは空になっていた。
Windowsのバージョン情報APIへ固定情報ブロックのポインタを渡していたことと、言語ID/コードページの順が逆になっていたことを修正した。
[MicrosoftのAPI仕様](https://learn.microsoft.com/en-us/windows/win32/api/winver/nf-winver-verqueryvaluew)に従い、元のバッファから翻訳テーブルを読み、言語ID→コードページの順に検索する。
修正後の実スキャンで、IntelliJとToolboxの2項目がJetBrainsフォルダになることを確認した。

## 承認された追加分類

名称はユーザー指定の「VRツール」「VR Games」「リモートデスクトップ」「Windows Dev tools」「OMEN」を採用した。
上記の152項目・11フォルダの集計は追加前の記録。追加後の実機検証は下記に追記する。

- VRツール: Meta Horizon Link、SlimeVR Server、Virtual Desktop Streamer、VRChat Creator Companion、SteamVR、XSOverlay。
- VR Games: VRChat、Beat Saber、Half-Life: Alyx。実機にはVRChatだけなので、2製品以上の条件によりVRChatは平置き。
- リモートデスクトップ: Parsec、Paperspace、Chromeリモートデスクトップ、リモートデスクトップ接続。
- Windows Dev tools: WinDbg、Dev Home、Windows App Cert Kit、Windows SDK、Visual Studio Installer、Application Verifier。
- OMEN: OMEN Audio Control、OMEN Gaming Hub。

同じアプリが複数表示される問題は分類の仕様ではなく検出処理のバグ。[PR #223](https://github.com/kamakiri1192/windows-launchpad-rust/pull/223)に分離した。
Steamはユーザー用と全ユーザー用のリンク、VRChat Creator Companionは二か所のリンク、Discordは直接起動とUpdater経由、FocuSeeはスタートアップ用リンクの混入が原因だった。
分類ルール自身は起動項目を削除しない。重複修正はその後main経由で統合した。

VLCの通常起動・スキン起動・設定リセットも単一製品の別操作であり、複数製品と見なして無理にフォルダを作らない。
ブラウザや単独のBlender、OBS、Everything等もトップレベルに残す。

## 検証

単体テストで管理アプリの収納、同製品の複数版だけのケース、別引数の除外、リセットと取り消し、翻訳テーブルの読取りを確認する。
GPUシナリオ `qa/initial_folder_layout_related_reset.json` はJetBrains・Git・Unity・Live2D・Maxonの生成→取り消し→設定リセット→取り消しを実際の入力処理で確認する。

### 追加分類後の確認

カタログv4の実機スキャン（重複修正PR適用前）では、152起動項目を維持し、66単体アプリ＋14フォルダ＝80項目になった。
VRツール7、リモートデスクトップ4、Windows Dev tools7、OMEN2。VRChatとDesktop Mateはそれぞれ分類内1製品のため平置き。
Application Verifierの2項目はWindowsツールからWindows Dev toolsへ移動した。
読み取れない既存ショートカット等によりスキャン完了状態はpartial。自動初期化では平置きを維持し、明示的な設定リセットでは取得できた集合に適用する従来の方針を保つ。

`qa/initial_folder_layout_categories_reset.json`で5分類の生成→取り消し→設定リセット→取り消しを確認した。
GPU連番のトップレベル数は6→11→6→11。表示名も画像で確認済み。
全ターゲットのテスト、Clippy、releaseビルドが成功。初期フォルダ配置の統合テストは27件成功。


main（bee06b9）統合後は161アプリを検出。デスクトップ補完によりUE5本体も含まれる。上記152項目の集計は統合前の履歴。
VRツールは重複整理により6項目となり、残りの既存分類も実機スキャンで確認した。

## 初期優先順の適用後（2026-09-25、カタログv5）

161アプリを一つずつ保持したまま、50単体アプリ＋15フォルダへ整理。Windowsアクセサリ15項目とPC管理31項目に分かれた。
PC管理にはコンピューターの管理、Windowsセキュリティ、問い合わせ、フィードバック、NVIDIA Control Panel、Global Configurationも入る。

先頭はエクスプローラー、Chrome、Edge、Discord、Store、設定、電卓、Adobe、OBS、Blender、Unreal Engine、VS Code、ChatGPT、ChatGPT Classic、Windowsアクセサリ。続いて制作系のフォルダと一般アプリ、最後にGame BarやGit・機器設定・開発補助・PC管理が並ぶ。

OneDrive等の読み取れない既存リンクによるpartial判定は残る。明示的な設定リセットでは検出済み集合にこの配置を適用する。
実機の全順序は `target/initial-order-installed.log`。GPUのリセット・取り消し・小画面検証は[実装記録](INITIAL_FOLDER_LAYOUT_IMPLEMENTATION.md)を参照する。
