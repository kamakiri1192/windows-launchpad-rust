# DB構成とマイグレーション管理の整理方針

状態: 承認済み・実装済み（2026-09-29）。検証結果は末尾に記録する。

## 目的

一般公開前に保存形式を整理し、今後のDB変更を `rusqlite_migration` で管理する。
独自の移行管理、旧形式へのフォールバック、配置の二重保存を解消する。
今回追加する製品機能はDB管理に必要な範囲に限定する。

## 変更前の構成

- Windowsは `cache.sqlite3` と `user-state.sqlite3` を利用している。
- macOSは `cache.sqlite3` にアイコン、設定、配置を保存している。
- Windowsでは `user-state.sqlite3` を優先し、存在しなければキャッシュ側の配置を読む。
- さらに旧 `app_order` / `hidden_ids` を読む経路があり、配置は両DBへ保存される。
- テーブル作成、列の存在確認、列追加、スキーマ番号更新をRustコードで直接管理している。

## 採用する構成

両OSとも、アプリが通常利用するDBを `launchpad.sqlite3` の1ファイルに統一する。
新しいファイル名で、旧DBを新形式として誤って扱うことを避ける。

| OS | 標準の保存先（非サンドボックス・非仮想化時の例） |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Launchpad\launchpad.sqlite3` |
| macOS | `~/Library/Application Support/Launchpad/launchpad.sqlite3` |

保存先は環境変数だけに依存せず、OSの保存先取得APIから解決する。

- Windows: `SHGetKnownFolderPath(FOLDERID_LocalAppData)` で取得した場所を使う。
- macOS: `FileManager` のユーザー領域の `Application Support` 取得APIを使う。
- 取得した場所に `Launchpad/launchpad.sqlite3` を配置する。

実行ファイルの隣やカレントディレクトリへのフォールバックは設けない。
所定の保存先を取得できない場合は、下記の起動時エラーと同じ扱いにする。
ストア版でOSが適用する専用コンテナや保存先の仮想化には従う。これは取得失敗時の代替保存先ではない。

DB内はアイコン用テーブルとユーザーデータ用テーブルに分ける。
ユーザーデータには設定、配置（並び順・フォルダ・非表示）、Windowsのアプリ識別対応情報を含める。
設定や配置のJSON表現は必要な範囲で維持し、今回全面的なテーブル分解は行わない。
Windows固有の識別情報のために別DBを作ることはしない。

設定・配置などをまとめて更新する操作は、同じDBのトランザクションで保存する。
アイコンのリセットはアイコン行だけを削除し、ユーザーデータを維持する。
アイコン抽出方式の変更を示す `EXTRACTION_VERSION` は、DB構造の版とは別の目的なので維持する。

## マイグレーション管理

- `rusqlite_migration` を採用する。
- `rusqlite_migration` 1.3.1（最低Rust 1.77、`rusqlite` 0.32.1依存）を採用し、ロックファイルに反映する。Rust 1.89・`rusqlite` 0.32は維持する。
- 初回公開用のテーブル定義を最初のマイグレーションにまとめる。
- SQLは番号順のファイルとしてGit管理し、実行ファイルに埋め込む。
- 起動時にDBを開き、通常の読み書き前に `to_latest` で未適用分を実行する。
- 適用済み番号と移行のトランザクション管理はライブラリに任せる。
- 一度公開したマイグレーションは変更せず、新しい変更を追加する。
- 保存済みJSONの変換が必要になった場合も、対応するマイグレーションの中で実行する。通常の読み込み処理に旧形式探索を追加しない。

## 削除する処理

- `user-state.sqlite3` がない場合に `cache.sqlite3` を読みに行く処理。
- 旧 `app_order` / `hidden_ids` から配置を復元する処理。
- 同じ配置を2つのDBへ保存する処理と、ファイル間の保存失敗を補う巻き戻し処理。
- 独自のスキーマ番号管理、列の有無による旧DB修復処理。
- DBが開けない場合にメモリDBへ切り替えて通常動作を続ける処理。
- DB保存先を取得できない場合に、実行ファイルの隣などへ保存する処理。
- `migration_layout` / `previous` に復旧用の状態を保持する仕組み。

既存の「初期配置の取り消し」はユーザー向け機能なので維持する。
これはDB障害時の復旧用記録とは別である。

## 失敗時の扱い

DB保存先の取得、DBのオープン、またはマイグレーションに失敗した場合は、通常起動に進まず非ゼロ終了する。
マイグレーション失敗時のロールバックはライブラリのトランザクションに任せる。
DBが変更前の状態に戻っても、新しいアプリに必要なスキーマへ更新できていないため、その起動は終了する。
ユーザー向けメッセージ、ダイアログ、通知、新しい診断画面は追加しない。
DBの自動削除・初期化、独自の復旧処理、同一起動中の再試行ループ、別DBへの切り替えは追加しない。
次回起動時には、通常どおり未適用のマイグレーションを実行する。
この方針は起動時のDB準備を対象とし、通常操作のエラー処理全般を作り直すものではない。

## 開発中データの扱い

一般公開前の旧形式への恒久的な対応は提供しない。
現在利用中の設定・配置は、実装時に保存状況を確認し、必要なデータを作業として一度だけ新DBへ移す。
その際はアプリを終了させた状態で旧DBを退避する。旧DBの削除は今回の作業に含めない。
一度限りの移し替えは通常の起動経路に組み込まない。
新DBには必要な現在状態を移し、旧形式の互換キーや復旧用履歴を持ち込まない。
アイコンは再抽出できるため移し替えを必須にしない。

## 今回の対象外

- 製品としてのバックアップ・復元機能。
- 自動アップデーター、インストーラー。
- 初回公開前の各旧版との互換性維持。
- 自動ダウングレード。
- DB以外の機能やUIの変更。

## 実装後の確認

- 空DBの初期化、初期化済みDBでの再実行、旧スキーマからの更新を確認する。
- 移行途中のSQL失敗で変更が確定しないこと、および新しすぎるDBを変更しないことを確認する。
- 設定・配置・識別情報の保存と再読み込み、関連変更の一括保存を確認する。
- アイコンリセットでユーザーデータが失われないことを確認する。
- 旧DBの探索・二重保存が通常起動経路からなくなったことを確認する。
- 保存先をOSのAPIから取得し、取得失敗時に別ディレクトリへDBを作らないことを確認する。
- 両OSの共通保存処理を検証し、OS固有のビルド・実機確認ができない部分は明記する。
- `cargo fmt --all -- --check`、`cargo test --all-targets --locked`、Clippy、releaseビルドを実施する。
- READMEとDB関連ドキュメントを最終実装に合わせて更新する。

## 参照

- [rusqlite_migration](https://docs.rs/rusqlite_migration/latest/rusqlite_migration/)
- [Migrations::to_latest](https://docs.rs/rusqlite_migration/latest/rusqlite_migration/struct.Migrations.html#method.to_latest)
- [Windows: SHGetKnownFolderPath](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath)
- [macOS: Protecting user data with App Sandbox](https://developer.apple.com/documentation/security/protecting-user-data-with-app-sandbox)
- 実装: `src/database.rs`、`migrations/001_initial.sql`、`src/icon_cache.rs`、`src/app/command.rs`、`src/app/initial_layout.rs`、`src/platform/paths.rs`

上記リンクは方針検討時の参照。実装では採用するバージョンのAPIと動作を確認する。

## 実施記録（2026-09-29）

- `rusqlite_migration` 1.3.1を導入。初期スキーマは `migrations/001_initial.sql`、適用順は `src/database.rs` で管理する。
- `icons` とユーザーデータ用 `kv` を同じDBに配置。`kv` の現在状態は `settings` / `launcher_state` / Windowsの `identity_aliases`。
- 旧DBの探索、列の自動修復、独自のスキーマ番号、ファイル間の二重保存・巻き戻し、メモリDBへの切り替えを削除した。
- 開発PCではアプリ終了後に旧DB2つをSQLiteのバックアップAPIで退避し、現在の設定・配置（65項目・15フォルダ）・識別情報を新DBへ移した。型付きの読み込み、配置の検証、保存後の読み戻し、`integrity_check` を確認した。
- 旧DBは元の場所に維持。退避コピーは同じ保存ディレクトリ内の `pre-unified-db-20260929-060532` に保管。一度限りの移し替えコードは製品に含めていない。
- Rust 1.89でDB・配置・保存先APIなどの関連188テストが成功。移行の原子性、再オープン後の再実行、将来版DBの拒否、アイコンリセット時のユーザーデータ保持、`LOCALAPPDATA` 未設定時のOS APIによる保存先解決を含む。
- `cargo test --bin launchpad-windows --lib --locked -j 1` が成功（ライブラリ605件、アプリ996件、計1,601件成功・8件ignore）。
- `cargo fmt --all -- --check` と全ターゲット・全機能のClippy（`-D warnings`）が成功。
- `cargo build --release --locked -j 1` は初回のコンパイラ異常終了後、再実行で成功。
- `cargo test --all-targets --locked -j 1` が最終的に成功（計1,687件成功・8件ignore）。途中で `rustc` / `link.exe` のアクセス違反等が発生したため再実行した。最終検証は固定のRust 1.89で実施しており、ツールチェーンやビルド設定は変更していない。
- macOSの保存先取得は使用バージョンの `objc2-foundation` のAPI定義を確認した。macOSネイティブビルド・実機起動はこのWindows端末では未検証。
