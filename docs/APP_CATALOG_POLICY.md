# Windowsアプリ一覧の判定と照合

候補列挙、表示対象の判定、アプリの同一性、現在の起動先、配置用の永続IDは別々に扱う。

## 表示対象

AppsFolderには通常のWebリンクや文書も含まれる。Shell由来であることやAUMIDが存在することは、アプリである証拠にはならない。

- 重複照合の前に、解決済み起動先のhttp(s)とInternet Shortcut（.url）を除く。
- ローカルの非実行ファイルは、起動先そのもののSystem.Kindを読む。document等の文書・メディア分類を除く。ショートカット自身のlink分類や表示名では判定しない。
- program/game分類、通常の実行ファイル、Store登録、Shell名前空間、ゲーム起動プロトコルは維持する。
- メタデータが取得できないことを、文書と分類できたこととは扱わない。

## 同一性

明示されたIDを優先する。IShellLinkのIPropertyStoreだけでIDが得られなければ、元ショートカットのShell property handlerにも問い合わせる。

IDのない候補の補完照合では、既存の実行ファイルと取得済みの引数が一致し、登録されたhostでないことを要求する。引数の大小文字・順序・内容は変更しない。異なる明示IDを、IDなしの候補を経由して統合しない。引数なしの既存aliasキーは維持する。

表示名、ベンダー名、更新フレームワーク名は判定に使わない。実行ファイルや引数が異なり、共有IDもない候補を同一と推測しない。

起動は現在のShell項目または元ショートカットに委譲する。解決済みexeのパスを、アプリの永続IDや将来の固定起動先として保存しない。配置・フォルダ・非表示の移行は既存の永続alias照合に任せる。

## 回帰確認

対象PCでは22件の文書・Webリンクを除外した。Sample Desktop Apps等は実行可能なサンプルではなく、紹介ページを開く.urlだった。Fork、Git Bash、Git CMD、Git GUI、Discord、Creator Companionは各1件、DesktopのみのUnreal EngineとWindows管理ツールも維持する。

実測fixture中のアプリ名は回帰試験の観測値であり、製品の分類・重複判定には使わない。合成テストでは、別名・別拡張子、未知の分類、複数のKind、異なる引数、競合するIDも確認する。

検索中の右クリックでは検索語・検索結果を維持し、Escと外側クリックはまずメニューを閉じる。既存のIME変換の確定は受け取り、メニューの背後では新しい検索入力を開始しない。

参考: [System.Kind](https://learn.microsoft.com/en-us/windows/win32/properties/props-system-kind)、[Using Kind Names](https://learn.microsoft.com/en-us/windows/win32/properties/building-property-handlers-user-friendly-kind-names)。

## Shell項目のアイコンとファイル表示

Win32アプリのAppsFolder項目をImageFactoryで取得すると、小さい実アイコンを大きな枠付きキャンバスに置いた画像が返る場合がある。枠も不透明なので、通常の余白除去では修復できない。Shell項目をPIDLとして解決し、SHGetFileInfo(SHGFI_PIDL)と従来のShell image listでアイコンを取得する。パッケージ登録があるアプリは、従来のImageFactoryでパッケージの画像を取得する。どちらも名前による分岐は行わない。

Windowsの抽出バージョン更新で旧画像を再取得し、配置用データは変更しない。Creator Companion、XSOverlay、x264設定ツール、拡大鏡、Discordを実機で比較した。設定とDev Homeはパッケージ経路を維持する。取得時間の単回観測は対象のデスクトップアプリで約4〜8msだったが、Shellキャッシュが温まる順序の影響があるため、起動時間の改善率とはしない。

| 以前の取得結果 | 修正後 |
| --- | --- |
| ![枠内に小さいアイコン](qa/desktop-icon-before.png) | ![アイコン本体](qa/desktop-icon-after.png) |

Explorerの表示はSHOpenFolderAndSelectItemsを使用する。実ファイルの起動先がある場合はそのファイルを選択し、プロトコル起動先しかない場合は元のShell項目を選択する。URIをファイルパスとしてExplorerのコマンドラインに渡さず、解決失敗を既定フォルダの表示で隠さない。実機ではUnityBugReporter.exeのインストール先と、Applications内のXSOverlayが選択されることを確認した。


## 起動先と提供元のアイコンを分離する

同一IDの候補を統合するとき、起動先を選ぶ優先順位をそのまま画像ソースに適用しない。Steam scannerが既存の選択処理で見つけた画像・exe・アプリバンドルを、独立した`provider_icon`（パス・更新時刻）として保持する。Shell項目で起動する場合も、この情報をicon workerへ渡す。表示名や起動URIの文字列では画像経路を切り替えない。

提供元画像の追加・移動・更新・消失で、該当アイコンのキャッシュを無効化する。画像ファイルが消失・破損していた場合は元のShell項目やショートカットのアイコンに戻す。キャッシュの列追加は既存行を維持し、提供元画像がない通常アプリと保存済みの配置を消去しない。

実機の統合後カタログ161件で、Steam由来の4件（XSOverlay、SteamVR、Desktop Mate、VRChat）が既存の高解像度ソースを使うことを確認した。4件とも取得結果は256×256、正規化後は128×128。XSOverlayではShell側の粗い画像からインストール済みexeの画像に戻ることを目視確認した。これは実アプリのスキャンからicon workerまでを通した取得画像であり、画面全体のキャプチャではない。

| Shell側の画像を使っていた状態 | 提供元画像を保持した状態 |
| --- | --- |
| ![Steamアイコン修正前](qa/steam-icon-before.png) | ![Steamアイコン修正後](qa/steam-icon-after.png) |

読み取り専用の実機検証: `cargo test --bin launchpad-windows report_catalog_provider_icons -- --ignored --nocapture`。取得結果は`target/steam-icon-probe/`に保存する。合成テストは異なる起動経路、別IDの分離、画像更新、提供元の欠落・無効化、画像破損、キャッシュの移行・更新を検証する。
