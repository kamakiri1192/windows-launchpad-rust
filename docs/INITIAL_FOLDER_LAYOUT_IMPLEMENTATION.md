# 初期フォルダ配置の実装・検証記録

対象仕様: [INITIAL_FOLDER_LAYOUT_SPEC.md](INITIAL_FOLDER_LAYOUT_SPEC.md)（PR #219）。
同梱プリセットと端末内の検出情報を使う初期配置を対象とする。

## 実装

- 初回スキャンのworkerが、配置案の計算とSQLite保存を済ませてからグリッドへ渡す。画像は従来の非同期経路を使う。
- 自動適用は新規の空データ領域が対象。既存利用者は、設定画面の「設定をリセット」で現在検出されているアプリに初期配置を適用し直せる。
- `launcher_state` JSONに、初期化理由、仕様/カタログ版、世代、改訂番号、作成したフォルダ、変更前後の論理順、取り消し用の検出情報を同居させる。同じSQLiteトランザクションで旧キーを移行する。
- 起動同一性はAppIdとは別。Windowsは解決済み実行先と取得済みの引数、MacはBundle ID、Steamはmanifest由来IDを使う。子のAppIdは一つも削除せず、異なる製品/付属機能と既知の起動対象がそれぞれ2種類以上ある場合だけフォルダ化する。
- Windowsの初回・設定リセットは日常の入口→作業→一般→管理・補助の優先順。同順位は小文字化した表示名と安定ID。macOSは名前順。通常グリッドでアプリとフォルダを混在させ、追加アプリは末尾へ入る。保存後の所属・優先順は再計算しない。
- 初期整理の取り消しは設定画面から行う。改名、移動、非表示、表示変更、検出情報の変化で取り消しを無効にする。アイコン画像だけの変更は対象外。
- 名前順の明示操作はフォルダの所属を維持する。「設定をリセット」は設定値と非表示状態を初期化し、初回と同じ生成器でフォルダと並び順を作り直す。設定と配置を一括保存し、新しい改訂番号の取り消し記録を持つ。
- `--reset-cache` はアイコン行だけを削除する。破損したDBを削除して作り直す経路は廃止し、元ファイルを残してメモリ上のキャッシュで起動する。
- スキャン成否はStart Menu列挙、ショートカット解決、MacのApplications、Steamで記録する。存在しない任意ディレクトリ/未導入Steamは正常な空結果。不完全な初回は `skipped_error` として平置きで確定する。後続の不完全スキャンでは既知のアプリを削除扱いにしない。
- 1,000件のGPU検証で見つかった既存のatlas拡張も修正。縦方向が8,192pxに達する場合は列数も増やし、既存の画素とUVを保つ。初期配置のためにアイコン数を制限しない。

## 同梱カタログ

仕様の19候補すべてに製品判定ルールを実装し、追加分類とWindowsアクセサリを加えた30定義に対応する。従来のWindowsツールはPC管理へ改め、日常のアクセサリと分けた。JetBrains Toolbox等の管理アプリ・付属ツールも同じ製品群へまとめる。OS別の対象製品、識別条件、除外境界と確認資料は [INITIAL_FOLDER_LAYOUT_CATALOG.md](INITIAL_FOLDER_LAYOUT_CATALOG.md) に記載する。

異なる製品が2種類以上ある場合に作成する。macOSのLibreOfficeのように一つのアプリとして配布されるものや、同じ製品の複数版だけがある場合はトップレベルに残す。

カタログの追加や言語変更は保存済みのフォルダを変更しない。新しいルールは初回配置と「設定をリセット」で適用する。

## 検証方法

- `tests/initial_folder_layout.rs`: 0/1/2対象、複数版、重複、別引数、Macの単一bundle、競合、表示設定、決定性、復元、取り消し、追加、50/200/1,000件。
- `src/initial_layout_cache_tests.rs`: 実SQLiteによる新規/既存/旧形式、破損、失敗注入とロールバック、再起動差分、キャッシュ削除、初期取り消し。
- `qa/initial_folder_layout.json`: 初期グリッド→設定から取り消す、というGPU描画とproductionのpointer処理。
- `qa/initial_folder_layout_reset.json`: 設定リセットによる再生成と、その後の取り消しを実際のボタン操作で検証する。
- `qa/initial_folder_layout_adobe_reset.json`: 実機で確認したAdobeの10項目、Creative Cloudクライアント、ブラウザを使い、生成・取り消し・リセット・再取り消しを検証する。
- `qa/initial_folder_layout_related_reset.json`: JetBrains・Git・Unity・Live2D・Maxonの管理/付属ツールを含む生成とリセット。
- `qa/initial_folder_layout_small.json`: 800×600・倍率1.0で初期グリッドを表示する。通常サイズのシナリオは1280×800・倍率1.5。
- `cargo test --test initial_folder_layout -- --nocapture` で配置計算時間を出力する。`cargo test --bin launchpad-windows report_initial_layout_for_installed_apps -- --ignored --nocapture` は端末のアプリを読み取るだけの診断で、保存や起動は行わない。
- デバッグ起動ログの `initial layout committed` と `initial grid interactive` で、保存完了とグリッドが操作可能になった時点を区別する。実行速度はスキャン、GPU、アイコンキャッシュ状態にも依存する。

Windowsのキャプチャは [EDIT_MODE_VISUAL_QA.md](EDIT_MODE_VISUAL_QA.md) と [GPU_SEQUENCE_QA.md](GPU_SEQUENCE_QA.md) に従う。生成画像・ログ・DBは `target/` 配下のみで管理し、コミットしない。

## 検証結果（2026-09-25、Windows）

- `cargo test --all-targets`、`cargo clippy --all-targets --all-features`、`cargo build --release` を実行。テスト成功、Clippy警告なし。
- GPU連番: 初期表示、設定からの取り消しとリセット、800×600の初期表示、英語・多ページ・50/200/1,000アプリ、既存 `folder_interactions.json` の編集・改名・ページング経路を確認。
- AdobeのGPU連番: 10制作アプリとCreative Cloudクライアントを1フォルダにまとめ、ブラウザは外へ残る。トップレベルの項目数は初期表示2→取り消し12→設定リセット2→再取り消し12。24件の初期配置テストも成功。
- 1,000アプリのfixtureは500個のSteamアプリと500個の不明アプリ。初回から501個のトップレベル項目となり、Steamフォルダには500個が入る。最初のグリッドから確定した配置を表示する。

| アプリ数 | 配置計算（debug、純粋関数） | プロセス起動→初回GPU画像の保存（release） |
| --- | --- | --- |
| 50 | 0.51ms | 2.41秒 |
| 200 | 1.28ms | 2.47秒 |
| 1,000 | 7.75ms | 2.67秒 |

各値は単発の参考測定。配置計算は全件Steam、GPU側は半数Steam・半数不明のfixture。
測定端末はWindows 11、Intel Core i7-10700K、NVIDIA GeForce RTX 3080。
後者はOSスキャンを省いた、操作可能な最初のグリッドをキャプチャするまでの時間であり、GPU初期化・readback・PNG保存・25msの監視間隔を含む。実データでのコールド起動時間や改善率を示すものではない。
実スキャン経路では既存のアイコンキャッシュ適用にUI側I/Oがあるが、追加した初期配置の計算・トランザクションはworker側で行う。

関連ツールと追加5候補を含む読取専用スキャンは152件、約1.21秒、配置候補計算は約17.51ms。11フォルダに72項目をまとめ、トップレベルは80アプリ＋11フォルダになる。詳細は[未分類アプリの確認記録](INITIAL_FOLDER_LAYOUT_AUDIT.md)を参照。
Start Menu列挙とOneDriveショートカット解決で不完全判定となるため、この検出結果を使う自動初期化は平置きとなる。「設定をリセット」では、この時点で検出できているアプリを対象にフォルダ配置を作り直す。

macOSの製品判定と配置生成はOSに依存しないテストで確認した。macOSネイティブビルド・実機表示・オフライン実機起動は、このWindows端末では検証していない。

- カタログv4: ユーザー指定名のVRツール／VR Games／リモートデスクトップ／Windows Dev tools／OMENを追加。AUMIDとSteamの固定IDに対応。初期配置テスト27件、全ターゲットのテスト、Clippy、releaseビルドが成功。
- GPUシナリオ `qa/initial_folder_layout_categories_reset.json`: 5分類とブラウザの6項目→取り消し11→設定リセット6→再取り消し11を確認。結果 `target/qa-sequences/initial-folder-layout-categories-reset-1790296561451`。
- アプリ重複の修正はPR #223の別ブランチに分離。その後のmain取り込みで統合済み。


## main統合（bee06b9）

- Windowsの新しいCatalogScan・永続的な同一アプリIDを維持し、重複整理後のスナップショットで初期配置を生成する。既存配置・不完全スキャン・操作済み配置の保護も維持する。
- user-state.sqlite3の配置を優先して読み、設定リセットと取り消しもこの保存先へ反映する。アイコンキャッシュ側の更新に失敗した場合は保存前の配置へ戻す。
- Steamの検出エラーはmainの詳細エラー形式を使用する。初期分類用の起動引数と、mainのprovider_iconの両方を保持する。
- 実機では161アプリを検出し、Unreal Engine（UE_5.8/UnrealEditor.exe）も一覧に含まれることを確認。分類はAdobe11、VRツール6、Windows Dev tools7、OMEN2、リモートデスクトップ4などを維持する。
- 全ターゲットのテストとClippyが成功。正規化済みIDの初期生成・既存配置の保持・不完全スキャン・二重初期化防止・取り消しの回帰テストを追加。

- 統合後のreleaseビルドとGPUシナリオも成功。`initial-folder-layout-categories-reset-1790333219021`で生成6項目→取り消し11→設定リセット6→取り消し11を確認。

## 初期優先順とWindowsフォルダの分割（仕様v2・カタログv5）

- `order.rs`で初回と設定リセット時の優先順を決める。設定・エクスプローラー・電卓・Store・ブラウザ・Discordを前へ、Adobe・OBS等の作業アプリを続け、管理・補助は後ろへ置く。
- WindowsアクセサリとPC管理を分離。完全なShell ID、SystemRoot内の限定パス・msc・引数で識別し、コンピューターの管理・セキュリティ・問い合わせ・フィードバック・NVIDIA設定・Global Configurationも収納する。
- 同じ製品の複数版が単体表示になる場合は代表1件を前方へ置く。他の版も保持する。分類できないアプリは一般帯で名前順。
- フォルダなしで並びだけ変わった場合も取り消し記録を保存する。既存配置やmacOSの初期順は維持する。
- `cargo test --all-targets`成功（library605、binary997、初期フォルダ28、初期優先順8を含む）。SQLiteで並びだけの保存・再読み込み・取り消し後の再適用防止を確認。Clippy、fmt、releaseビルドも成功。
- 実機スキャンは161アプリを維持し、50単体＋15フォルダの65項目。Windowsアクセサリ15、PC管理31、Adobe11。先頭はエクスプローラー→Chrome→Edge→Discord→Store→設定→電卓→Adobe→OBS→Blender→Unreal Engine→VS Code→ChatGPT。`target/initial-order-installed.log`に全順序を記録。
- GPUシナリオ `qa/initial_layout_order_reset.json`：生成16項目→取り消し20→設定リセット16→再取り消し20。件数だけでなく各フレームのIDと表示名の並びを確認。結果 `initial-layout-order-reset-1790335009117`。
- `qa/initial_layout_order_only_reset.json`：2項目のまま「設定→一般」と名前順を切り替え、設定の取り消し・リセットが機能することを確認。結果 `initial-layout-order-only-reset-1790335015647`。
- `qa/initial_layout_order_small.json`：800×600でも同じ論理順で描画されることを画像とmanifestで確認。結果 `initial-layout-order-small-1790335022132`。

GPU結果は `target/qa-sequences/` 配下。fixtureは識別情報を与えたダミーアイコンを使い、実機のアプリ情報の検証は読取専用スキャンで行う。
