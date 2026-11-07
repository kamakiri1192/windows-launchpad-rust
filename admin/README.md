# Liquid Start Console

Windows / macOS版Launchpadの分類データを運用する、配信元専用の管理コンソールです。

通常の入口は **収集・確認** です。アプリ名や識別IDを1件ずつ入力する運用を前提にしていません。

1. 管理者のMac / Windowsからアプリとアイコンを一括収集。
2. 既存ルールと公式カテゴリを照合し、登録・アイコン補完の候補を作成。
3. 判定済みはまとめて下書きへ保存。未分類だけフォルダを選択するか、MCPで調査・補完。
4. 管理者が公開すると静的JSONを更新。過去版への差し戻しも新しいバージョンとして公開。

Mac App Storeの公開検索は、保存した検索テーマからCronで定期収集できます。公式カテゴリを持つ候補を下書きへ自動保存する設定もあります。**公開は別操作**です。公開検索はMac App Storeの検索結果が対象で、Windowsやストア外の全アプリを網羅するクローラーではありません。Windowsの識別情報は実端末の収集とMCPで補完します。

## ローカル起動

Node.js 22.12以降、PHP 8.2以降（curl / gd / session / fileinfo）を使用します。

```sh
cd admin
npm ci
# パスワードは16文字以上。引数やシェル履歴に残さず標準入力から渡す。
# 開発専用: このコマンドの後、パスワードを入力してEOFを送る。
LAUNCHPAD_LOCAL_SETUP=1 php scripts/setup.php
npm run api
# 別のターミナル
npm run dev
```

`http://127.0.0.1:5173` にアクセスします。PHPサーバーは127.0.0.1だけにbindします。ローカルHTTP許可は実際の接続元がloopbackの場合に限ります。

## 端末から一括収集

リポジトリのルートで実行します。既存のスキャナーとアイコン抽出処理を利用し、ウィンドウやGPUを起動せず終了します。

```sh
# macOS
cargo run --release -- --export-catalog-inventory inventory.json
# Windows (PowerShell)
.\target\release\launchpad-windows.exe --export-catalog-inventory inventory.json
```

管理画面の「端末の収集JSONを取り込む」で選択します。全件を小さなバッチに分割し、同じ識別情報は重複登録しません。既存アプリのアイコンが空の場合は補完します。

ファイルは明示操作でローカルに作成され、自動送信しません。パス、起動引数、ユーザー名、マシン名、インストール場所は出力しません。名前、OSの公開製品識別情報、アイコン、分類候補を含みます。収集対象のアプリの存在自体はファイルに含まれるため、管理者が内容を確認して取り込みます。端末に未導入のアプリはMCPの `import_inventory` でも登録候補にできます。

同じ製品の別バージョンは既存の製品キーを共有します。共有ホスト実行ファイルや引数付きの未知のWindowsショートカットから、曖昧なルールを自動生成しません。

## Xサーバーへの配置

```sh
npm run package
```

生成先はリポジトリの `target/admin-package/` です。次の構造で配置します（管理用サブドメイン推奨）。

```text
DOMAIN/
  public_html/             ← package/public_html の内容のみ公開
    index.html
    assets/
    api/index.php
    api/mcp.php
    catalog/               ← PHPの実行ユーザーが書き込み可能
  server/                  ← package/server。公開ディレクトリの外
    bootstrap.php
    config.php             ← config.example.phpをコピーして設定
    data/catalog-v1.json
  launchpad-storage/       ← 公開ディレクトリの外、PHPが書き込み可能
```

1. PHP 8.2以降、curlとGDを有効にし、HTTPSを設定。
2. `server/config.example.php` を `server/config.php` にコピー。パスと `password_hash` を設定し、権限600。
3. `launchpad-storage` と `public_html/catalog` にPHPの書き込み権限を付与（例: 所有者をPHP実行ユーザーにして750/770）。777にしない。
4. `public_html/.htaccess` を含めてアップロード。分類JSONはApache/nginxの静的配信、管理APIは `no-store`。
5. ログインして初回公開。`https://YOUR_DOMAIN/catalog/catalog-v1.json` を認証なしで取得できることを確認。
6. トークンを発行し、MCPの接続も確認。

サブディレクトリ配置の場合もViteの相対baseとHashRouterで動作します。ただし `api/*.php` が参照する `../../server` は必ず非公開側へ向けて変更してください。`server/` や `storage/` をそのまま公開ルート配下へ置かないでください。

配布パッケージにローカル設定・パスワード・収集済み個人データを含めません。既存の `storage` と公開 `catalog` は再デプロイ時に上書き・削除しないでください。設定、storage、公開catalogはセットでバックアップします。PHP以外の常駐サーバー、Node.js実行環境、データベースサービスは本番で不要です。

### 定期収集

管理画面で検索テーマ、国、自動下書き保存の有無を設定します。XサーバーのCronから1日1回、次のスクリプトを実行します。PHP CLIのパスは契約サーバーのPHPバージョンに合わせます。

```sh
/usr/bin/php8.3 /home/ACCOUNT/DOMAIN/server/collect.php
```

検索結果は24時間キャッシュします。検索テーマは最大10件、各テーマ最大12件。ブラウザ収集は約15秒単位で処理を分割し、取得済み位置から継続します（個々の外部通信は最大10秒）。Cronは全バッチを継続して処理します。途中失敗は候補を保持して記録され、並行実行はロックで拒否します。収集だけで配信用JSONを変更することはありません。

## Rustアプリの配信先設定

配信元URLが決まったら、リリースビルド時に指定します。

```sh
LAUNCHPAD_CATALOG_URL=https://YOUR_DOMAIN/catalog/catalog-v1.json cargo build --release
```

Windows PowerShellは `$env:LAUNCHPAD_CATALOG_URL = 'https://…/catalog/catalog-v1.json'` を設定してからビルドします。実行時の同名環境変数で上書きもできます。未設定では同梱JSONだけで動作します。実在しない配信先へのリクエストは行いません。

- HTTPS限定、リダイレクトなし、全体8秒、最大2 MiB。
- 同梱→検証済みキャッシュ→バックグラウンド更新。ネットワーク待ちで起動を止めません。
- 成功後は24〜30時間に1回。初回・期限切れで起動したとき、スリープ等で予定を5分以上過ぎたときは30秒〜30分に分散。次の取得予定を端末に保存し、再起動で未到来の予定をリセットしません。
- 失敗時は約1時間から指数的に再試行間隔を延ばし、最大24時間＋25%のランダムな遅延。正常取得でリセット。ETag/304に対応。
- 未対応スキーマ、矛盾する識別ルール、古い版、同じ版番号での内容変更は拒否。
- 新しいデータを取得しても、既存の配置を並べ替えません。初回配置または明示的な設定リセット時に利用します。
- 初回起動でまだ取得できていない場合は同梱版を使用します。ダウンロード完了後は次の初期配置操作から利用可能です。
- 既存の「異なる製品と起動先が2つ以上でフォルダを作成」「元に戻す」「配置を維持」の制約を継承。
- 利用者のアプリ一覧・アイコンを送信しません。端末のランチャー画像は引き続きOSから抽出します。管理アイコンは管理画面のための資料です。

### 利用者が増えたときの配信

一般利用者は認証不要の共通JSONを取得し、端末内で分類します。利用者ごとのPHP処理・DB検索・アイコン転送はありません。管理者のログイン・MCP・収集APIは、一般利用者の配信経路に含まれません。

公開JSONは空白・改行を省いて配信します。同梱238件を公開した場合は約64 kB（非圧縮）。仮に10万台が1日1回、毎回全文を取得しても本文は約6.4 GB/日です。実際には変更なしの応答は304で本文を返しません。この試算はカタログの大きさで変わり、HTTP/TLSの通信量や新規インストールは含みません。サーバーの収容人数を保証する値ではありません。

大規模配信では公開JSONの前段にCDNを配置し、端末へ配る `LAUNCHPAD_CATALOG_URL` をそのHTTPS URLにします。キャッシュ対象は `/catalog/catalog-v1.json` と `/catalog/icons/` に限定し、`/api/` と管理画面の認証応答はキャッシュしません。公開JSONのCache-Controlは1時間、アイコンは内容ハッシュ付きURLで1年。ETagを保持し、正常配信時は更新から最大約1時間のキャッシュ遅延と端末側の取得間隔を運用上許容する設計です（[HTTPキャッシュの仕様](https://www.rfc-editor.org/rfc/rfc9111.html)）。CDNやXサーバー本番の設定はこのPRでは行っていません。

本番公開前に、想定台数・同時起動数・カタログ最大サイズに応じて、CDNのキャッシュ命中率、オリジンへのリクエスト数、転送量、429/5xxを計測します。配信中の公開操作で途中のJSONが見えないこと、ETag/304、配信停止中のオフライン動作も確認します。ローカル試験はスケジュール分散・再起動・障害時の再試行を検証するもので、Xサーバー上の大規模負荷試験の代わりではありません。

## MCP / Codex

「MCP接続」で90日有効のトークンを発行し、安全な環境変数に設定します。トークンは一度だけ表示され、サーバーではSHA-256だけを保持します。再発行・失効は即時反映されます。

```toml
[mcp_servers.launchpad]
url = "https://YOUR_DOMAIN/api/mcp.php"
bearer_token_env_var = "LAUNCHPAD_MCP_TOKEN"
```

Streamable HTTPのJSON応答モードで、MCP `2025-11-25` / `2025-06-18` / `2025-03-26` に対応。公式TypeScript SDKクライアントで相互接続試験しています。認証は配信元専用のBearerトークンです。OAuthログインを要求するクライアント向けのOAuth認可サーバーは含みません。

| ツール | 用途 |
|---|---|
| `list_inbox` | 収集済みの未確認候補を取得 |
| `import_inventory` | 端末情報・調査した正確なIDをまとめて候補化 |
| `accept_inbox` | 選んだ候補と分類先を下書きへ反映 |
| `collect_sources` | 保存済み条件で公開ストアを収集。`has_more` がtrueなら継続実行 |
| `get_catalog` | 下書き全体とrevisionを取得 |
| `save_catalog` | revisionを検証して下書きを保存 |
| `list_releases` | 配信中の版と履歴を取得 |
| `publish_catalog` | 下書きを公開 |
| `restore_release` | 過去版を新しい版番号で再公開 |

例: 「収集候補を見て、正確な識別情報と用途を確認できるものを既存の分類へ入れて。下書き保存まで」「差分を確認して、問題なければ公開して」。MCPもブラウザも同じサービス層と検証処理を使います。公開・差し戻しは更新系ツールとして宣言し、説明にも明示的な公開指示が必要であることを記載しています。

## 構造と拡張

- `src/app`: 認証後のシェル、ナビゲーション、ルート。画面を遅延読み込み。
- `src/features`: auth / catalog / collection / publication / integrations の機能単位。
- `src/shared`: HTTPクライアント、共通UI。Radix Themesでフォーカス管理・ダイアログ・入力を統一。
- TanStack Queryはサーバー状態、CatalogContextは編集中の下書きと編集開始時のrevisionを管理。
- `server/Contract.php`: 配信用スキーマ・上限・重複の検証。
- `CatalogStore`: 非公開下書き、排他制御、不変リリース、公開ファイルのatomic replace。
- `Inbox` / `Collector` / `Icons`: 収集、候補化、正規化、承認。`McpServer` とHTTPルートは薄いアダプター。
- `assets/catalog/catalog-v1.json`（リポジトリルート）: 既存238件・52フォルダを移行した唯一の同梱データ。
- Rust側のOS固有の厳密な判定アルゴリズム（Officeの起動引数など）とトップレベル維持の保護はコードに残します。一般的な識別ルール・分類先の追加にはアプリの再ビルドは不要です。

新機能は `features/<feature>` とルート、サーバーのサービスを追加します。配信用スキーマを壊す変更はschema_versionを上げ、既存アプリが確実に拒否できるようにします。

## 検証

```sh
npm run test
npm run format:check
npm run build
npm run test:e2e
```

E2Eは**専用の一時設定**で実行してください。通常の保存データを指さず、`scripts/test-config.php` と `LAUNCHPAD_ADMIN_CONFIG` を使います。CIはこの分離を行います。ローカルはChrome、CIはPlaywright Chromiumを使用。

ルート側では `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --locked -- -D warnings`、`cargo test --all-targets --locked`、`cargo build --release --locked`。公開ファイルを `cargo run --example catalog_validate -- catalog.json` でRust側と突き合わせられます。

参考: [Radix Themes](https://www.radix-ui.com/themes/docs/overview/getting-started)、[ReactとVite](https://react.dev/learn/build-a-react-app-from-scratch)、[XサーバーPHP](https://www.xserver.ne.jp/manual/man_program_php_ver.php)、[Apple Search API](https://performance-partners.apple.com/search-api)、[MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)。
