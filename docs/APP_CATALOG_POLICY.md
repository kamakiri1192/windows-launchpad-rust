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
