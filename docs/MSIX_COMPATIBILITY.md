# MSIX版の互換性確認

Liquid Startは、従来どおりWin32アプリとして動作します。MSIX版では、実行ファイルにWindowsデスクトップ向けのマニフェストを付け、Microsoft Storeから配布します。パッケージ化に伴う実行時コードやアプリ内部の識別子、Steam版の起動方法、GitHub ZIP版の構成は変更していません。

## 機能ごとの確認項目

| 機能 | 現在の実装 | MSIX版で確認すること |
| --- | --- | --- |
| `Win + Space` のグローバルショートカット | フル信頼（通常のデスクトップアプリと同じ権限）で動作し、`SetWindowsHookExW` で `WH_KEYBOARD_LL` を登録します。 | Store版をインストールした状態で、フックの登録、キー入力の検出と抑止、終了時の解除をWindows 10/11で確認します。 |
| 通知領域のアイコン | 専用のメッセージ処理スレッドから `Shell_NotifyIconW` を呼び出します。 | アイコンとメニューの表示、アプリ終了後の再起動、エクスプローラー再起動後の復帰を確認します。 |
| Liquid Glassの画面取り込み | Windows Graphics CaptureとDirect3Dを連携させて画面を取り込みます。 | キャプチャに対応しているか、必要な許可画面が表示されるか、映像が更新されるか、終了時にリソースを解放するかを確認します。 |
| インストール済みアプリとスタートメニュー | `shell:AppsFolder` と、ユーザー別・全ユーザー共通のスタートメニュー用フォルダーを調べます。 | 通常のWin32アプリ、パッケージアプリ、Storeアプリが一覧に表示され、起動できることを確認します。Liquid Start自身が一覧に現れる場合も確認します。 |
| Steamアプリ | レジストリとディスク上のSteamライブラリ情報を調べます。 | レジストリからの検出と、各ライブラリへのアクセスを確認します。別ドライブやアクセス制限のある場所にあるライブラリも対象です。 |
| SQLiteの保存データ | `%LOCALAPPDATA%\Launchpad\cache.sqlite3` を使います。`LOCALAPPDATA` が使えない場合は実行ファイルと同じフォルダーを使います。 | 既存データの読み書き、新規作成、パッケージ更新後の保持、アンインストール後の扱いを確認します。Store版・GitHub ZIP版・Steam版の間でキャッシュを共有できるかも確認します。 |
| 多重起動の防止 | `Local\LaunchpadWindowsSingleInstance` という名前付きミューテックスを使います。 | MSIX版と通常の実行ファイルを同時に起動しても、既存の多重起動防止が働くことを確認します。 |

## 保存データの扱い

マニフェストでは `Windows.FullTrustApplication` を指定しています。Microsoftの説明では、フル信頼アプリがユーザープロファイル内の `AppData` に書き込む場合、通常の保存先をそのまま使います。一方、パッケージごとの保存先への振り分けは、仮想化されたアプリについて説明されています。Liquid Startはフル信頼で動作するため、既存の `%LOCALAPPDATA%\Launchpad\cache.sqlite3` を引き続き使う想定です。

詳しくは、[MSIXのコンテナー化の仕組み](https://learn.microsoft.com/en-us/windows/msix/msix-containerization-overview)と[パッケージ化されたデスクトップアプリの動作](https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-behind-the-scenes)を参照してください。対応するWindowsの各バージョンで既存データを読み書きし、Store版・GitHub ZIP版・Steam版の間で共有できることを確認します。この確認が終わるまでは、MSIX版のリリース検証は完了としません。

診断ログも `%LOCALAPPDATA%\Launchpad` に保存します。Cargoのcrate名、`LAUNCHPAD_*` 環境変数、ミューテックス名、データベース互換性ID、QA用の識別子、Rustのモジュール名は変更していません。

既存のキャッシュは削除も移行もしません。実際のユーザーデータで試す前に、既存キャッシュのコピーを使って読み書きや更新後の動作を確認してください。

## パッケージ化による変更範囲

アプリのパッケージ識別情報と `runFullTrust` の宣言はMSIXマニフェストに追加します。実行ファイルは引き続き `x86_64-pc-windows-msvc` 向けです。Steam版やGitHub ZIP版は通常のWin32アプリとして起動するため、MSIXのプロセス環境や `WindowsApps` 内の配置には依存しません。

MSIXのビルドと署名の手順は[Windows向けパッケージングの説明](../packaging/windows/README.md)を参照してください。
