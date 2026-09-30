# Macアプリ全件の分類監査

確認日: 2026-09-30。Mac実機の `~/Applications`、`/Applications`、`/System/Applications` とSteamライブラリを読み取り、アプリ名・Bundle ID・起動先・用途を確認した。

**270個のアプリ束と1件のSteam manifestを確認。表示対象は217件で、211件を33フォルダへ収納、6件は意図して直接表示。未判定は0件。**

アプリ束の内訳は表示対象217件、Bundle ID等の重複44件、`LSBackgroundOnly=true` の7件、通常スキャナーの深さ上限を超えるMocha AEの旧版2件。Steam manifestのDesktop Mateは既存ショートカットと同じ `steam:3301060` として統合する。検出結果を一度ずつ保持し、分類のためにアプリを削除・非表示にしていない。

スキャン対象のアプリ束の内部には再帰しない。これはプラグインや埋め込みヘルパーを独立アプリとして合成しない既存のスキャン契約。監査モードは通常の深さ上限を超えるアプリ束も記録するが、通常の表示対象は増やさない。ユーザー名を含むホームのパスは `~` に置換している。

分類器から逆算した抜粋ではなく、実機全件の用途と期待する所属を固定した [217件の期待値](../tests/fixtures/macos_catalog_full_observed.json) と [270個のアプリ束の監査記録](../tests/fixtures/macos_bundle_inventory_observed.json) を保存した。全件の所属・重複なし・リセット・保存後の取り消しはテストで確認する。

## フォルダ別の全件内訳

| 所属 | 件数 | 全アプリ |
| --- | ---: | --- |
| 3D・アバター | 9 | 3teneFREE_beta、Blender、Looking Glass Bridge 2.4.5、nanoem、Quixel_Mixer_2022.1.1_Beta、VRMポージング・モバイル、VRoid Studio、Webcam Motion Capture、Webcam Motion Receiver |
| Adobe Creative Cloud | 24 | Adobe After Effects 2024、Adobe After Effects Render Engine 2026、Adobe Audition 2026、Adobe Bridge 2025、Adobe Content Synchronizer、Adobe Creative Cloud Diagnostics、Adobe Illustrator 2026、Adobe Media Encoder 2024、Adobe Media Encoder 2025、Adobe Media Encoder 2026、Adobe Photoshop 2026、Adobe Premiere Pro 2024、Adobe Premiere Pro 2025、Adobe Premiere Pro 2026、After Effects、Contact Sheets、Creative Cloud、Creative Cloud Desktop App、Creative Cloud Installer、Creative Cloud Uninstaller、InDesign、Make Calendar、Mocha AE、Web Gallery |
| Affinity | 2 | Affinity Designer、Affinity Photo |
| AI・音声入力 | 6 | Chatbot UI、ChatGPT、LM Studio、MacGPT、Perplexity、superwhisper |
| Apple文書アプリ | 4 | Keynote、Numbers、Pages、Pages Creator Studio |
| CLIP STUDIO | 4 | AggregateMdul、CertMdul、CLIP STUDIO、CLIP STUDIO PAINT |
| Google Workspace | 3 | Google Docs、Google Sheets、Google Slides |
| JetBrains | 8 | Air、GoLand、IntelliJ IDEA Ultimate、JetBrains Rider、JetBrains Toolbox、PhpStorm、PyCharm Professional Edition、RustRover |
| Live2D Cubism | 2 | Live2D Cubism Editor 5.3、Live2D Cubism Viewer (for OW) |
| macOSユーティリティ | 27 | AirMacユーティリティ、Audio MIDI設定、Automator、Bluetoothファイル交換、Boot Campアシスタント、ColorSyncユーティリティ、DaisyDisk、Digital Color Meter、Font Book、Grapher、Mission Control、RunCat、Siri、Time Machine、VoiceOverユーティリティ、アクティビティモニタ、イメージキャプチャ、コンソール、システム情報、ショートカット、スクリプトエディタ、ターミナル、ディスクユーティリティ、パスワード、ヒント、プリントセンター、移行アシスタント |
| Macアクセサリ | 6 | QuickTime Player、スクリーンショット、スティッキーズ、テキストエディット、ボイスメモ、辞書 |
| Macカスタマイズ | 9 | BetterTouchTool、LauncherTurbo、Launchpad、Launchpad、LaunchpadPlus、NotchNook、ScreenNote、ScreenPointer、Scroll Reverser |
| Maxon | 6 | c4dpy、Cinema 4D、Cinema 4D Team Render Client、Cinema 4D Team Render Server、Commandline、uninstall |
| PDF | 3 | Acrobat、Distiller、プレビュー |
| Steamライブラリ | 2 | Desktop Mate、Sid Meier's Civilization VI |
| Steinberg | 8 | Cubase LE AI Elements 13、eLicenser Control Center、Groove Agent SE、HALion Sonic、Steinberg Activation Manager、Steinberg Download Assistant、Steinberg Library Manager、Uninstall Steinberg Download Assistant |
| Unity | 3 | Unity、Unity Bug Reporter、Unity Hub |
| VOCALOID | 5 | VOCALOID Authorizer、VOCALOID Authorizer Uninstaller、VOCALOID Uninstaller、VOCALOID4 API Uninstaller、VOCALOID6 Editor |
| ゲーム | 3 | NTE、チェス、トリッカル・もちもちほっペ大作戦 |
| デザイン | 3 | Cavalry、Figma、Pencil |
| デバイス連携 | 3 | iPhoneミラーリング、ホーム、探す |
| ファイル転送・同期 | 5 | FileZilla、Google Drive、Mountain Duck、Nextcloud、RemoteDrive |
| メディア再生 | 6 | TV、VLC、YouTube Music、ブック、ポッドキャスト、ミュージック |
| メモ・予定・作業 | 10 | Goodnotes、Koucha、Notion、Notion Web Clipper、カレンダー、チアプロ - 集中・継続を応援、フリーボード、メモ、リマインダー、時計 |
| リモートデスクトップ | 3 | Barrier、Chrome Remote Desktop Host Uninstaller、画面共有 |
| 仮想マシン | 2 | Parallels Desktop、VMware Fusion |
| 写真・画像 | 5 | ComfyUI、Photo Booth、Playground、アニメ画像に変換する君、写真 |
| 映像・配信・演出 | 4 | iMovie、Lightkey、OBS Studio、obsctl |
| 生活情報 | 4 | News、マップ、天気、株価 |
| 連絡・通話 | 8 | Discord、FaceTime、LINE、X、zoom.us、メッセージ、メール、連絡先 |
| 開発ツール | 14 | Antigravity、Code、Cursor、Fork、iTerm2、iTermBrowserPlugin、Kiro、OpenCode、RapidAPI、Remote Codetrol Server、TablePlus、Visual Studio、Xcode、ZCode |
| 音声・歌声合成 | 6 | Crypton Product Manager、Piapro Studio NT、sbv2-gui、Synthesizer V Studio 2 Pro、Synthesizer V Studio Basic、Synthesizer V Studio Pro |
| 音楽制作 | 4 | Capo、GarageBand、MIDI Monitor、TouchOSC |
| 直接表示 | 6 | App Store、Google Chrome、Safari、Steam、システム設定、計算機 |

## 表示対象217件の所属と理由

製品セットに属する項目は、そのセットを優先する。残りは制作・開発・通信・文書・再生・OS管理などの用途で分類した。

| アプリ | Bundle ID / Steam ID | 所属 | 判断理由 |
| --- | --- | --- | --- |
| 3teneFREE_beta | `jp.plusplus.virtualyoutuber` | 3D・アバター | 3Dアバターの表示・配信 |
| Blender | `org.blenderfoundation.blender` | 3D・アバター | 3Dモデル・アニメーション制作 |
| Looking Glass Bridge 2.4.5 | `com.lookingglassfactory.bridge` | 3D・アバター | 3Dコンテンツ表示用のLooking Glass連携 |
| nanoem | `com.github.nanoem` | 3D・アバター | 3Dモデルのモーション制作 |
| Quixel_Mixer_2022.1.1_Beta | `unity.Quixel.Mixer` | 3D・アバター | 3Dモデル用テクスチャ制作 |
| VRMポージング・モバイル | `com.soarhap.vrmposing` | 3D・アバター | VRMアバターのポーズ作成 |
| VRoid Studio | `net.pixiv.vroid.macosx` | 3D・アバター | 3Dアバター制作 |
| Webcam Motion Capture | `Webcam Motion Capture` | 3D・アバター | Webカメラのモーションキャプチャ |
| Webcam Motion Receiver | `info.kwcl.WebcamMotionReceiver` | 3D・アバター | アバター用のモーション受信 |
| Adobe After Effects 2024 | `com.adobe.AfterEffects` | Adobe Creative Cloud | 映像の合成・エフェクト制作 |
| Adobe After Effects Render Engine 2026 | `com.adobe.AfterEffectsRenderEngine` | Adobe Creative Cloud | After Effects付属のレンダリング機能 |
| Adobe Audition 2026 | `com.adobe.Audition` | Adobe Creative Cloud | 音声の録音・編集 |
| Adobe Bridge 2025 | `com.adobe.bridge` | Adobe Creative Cloud | 制作素材の管理 |
| Adobe Content Synchronizer | `com.adobe.accmac` | Adobe Creative Cloud | Adobeのコンテンツ同期補助 |
| Adobe Creative Cloud Diagnostics | `com.adobe.cc.Adobe-Creative-Cloud-Diagnostics` | Adobe Creative Cloud | Creative Cloudの診断 |
| Adobe Illustrator 2026 | `com.adobe.illustrator` | Adobe Creative Cloud | ベクターグラフィック制作 |
| Adobe Media Encoder 2024 | `com.adobe.ame.application.24` | Adobe Creative Cloud | 映像・音声のエンコード |
| Adobe Media Encoder 2025 | `com.adobe.ame.application.25` | Adobe Creative Cloud | 映像・音声のエンコード |
| Adobe Media Encoder 2026 | `com.adobe.ame.application.26` | Adobe Creative Cloud | 映像・音声のエンコード |
| Adobe Photoshop 2026 | `com.adobe.Photoshop` | Adobe Creative Cloud | 画像編集 |
| Adobe Premiere Pro 2024 | `com.adobe.PremierePro.24` | Adobe Creative Cloud | 動画編集 |
| Adobe Premiere Pro 2025 | `com.adobe.PremierePro.25` | Adobe Creative Cloud | 動画編集 |
| Adobe Premiere Pro 2026 | `com.adobe.PremierePro.26` | Adobe Creative Cloud | 動画編集 |
| After Effects | `com.adobe.AfterEffects.application` | Adobe Creative Cloud | 映像の合成・エフェクト制作 |
| Contact Sheets | `com.apple.ScriptEditor.id.Contact-Sheets` | Adobe Creative Cloud | Illustrator同梱のコンタクトシート生成サンプル |
| Creative Cloud | `com.adobe.acc.AdobeCreativeCloud` | Adobe Creative Cloud | Adobe制作製品の導入・管理 |
| Creative Cloud Desktop App | `com.adobe.Creative-Cloud-Desktop-App` | Adobe Creative Cloud | Creative Cloudの管理入口 |
| Creative Cloud Installer | `com.adobe.Install` | Adobe Creative Cloud | Creative Cloudのインストール管理 |
| Creative Cloud Uninstaller | `com.adobe.ACCC.Uninstaller` | Adobe Creative Cloud | Creative Cloudの削除管理 |
| InDesign | `com.adobe.InDesign` | Adobe Creative Cloud | ページレイアウト制作 |
| Make Calendar | `com.apple.ScriptEditor.id.Make-Calendar` | Adobe Creative Cloud | Illustrator同梱のカレンダー生成サンプル |
| Mocha AE | `com.borisfx.mocha4ae_adobe` | Adobe Creative Cloud | After Effects同梱のトラッキング・マスク制作 |
| Web Gallery | `com.apple.ScriptEditor.id.Web-Gallery` | Adobe Creative Cloud | Illustrator同梱のWebギャラリー生成サンプル |
| Affinity Designer | `com.seriflabs.affinitydesigner` | Affinity | ベクターグラフィック制作 |
| Affinity Photo | `com.seriflabs.affinityphoto` | Affinity | 写真・画像編集 |
| Chatbot UI | `com.google.Chrome.app.hbblfifohofgngfbjbiimbbcimepbdcb` | AI・音声入力 | Chatbot UIのWebアプリ入口 |
| ChatGPT | `com.openai.codex` | AI・音声入力 | インストール済みChatGPT・CodexのAIアシスタント |
| LM Studio | `ai.elementlabs.lmstudio` | AI・音声入力 | ローカル言語モデルの実行・対話 |
| MacGPT | `com.goodsnooze.MenuGPT` | AI・音声入力 | メニューバーのAIチャット |
| Perplexity | `ai.perplexity.mac` | AI・音声入力 | AI検索・対話 |
| superwhisper | `com.superduper.superwhisper` | AI・音声入力 | AIによる音声入力 |
| Keynote | `com.apple.iWork.Keynote` | Apple文書アプリ | プレゼンテーション作成 |
| Numbers | `com.apple.iWork.Numbers` | Apple文書アプリ | 表計算 |
| Pages | `com.apple.iWork.Pages` | Apple文書アプリ | 文書作成（別版も同製品） |
| Pages Creator Studio | `com.apple.Pages` | Apple文書アプリ | 文書作成（別版も同製品） |
| AggregateMdul | `jp.co.celsys.AggregateMdul` | CLIP STUDIO | CLIP STUDIO付属モジュール |
| CertMdul | `jp.co.celsys.CertMdul` | CLIP STUDIO | CLIP STUDIOの認証補助 |
| CLIP STUDIO | `jp.co.celsys.CLIPSTUDIO` | CLIP STUDIO | CLIP STUDIOの制作環境管理 |
| CLIP STUDIO PAINT | `jp.co.celsys.CLIPSTUDIOPAINT` | CLIP STUDIO | イラスト・漫画・アニメーション制作 |
| Google Docs | `com.google.drivefs.shortcuts.docs` | Google Workspace | Googleの文書作成入口 |
| Google Sheets | `com.google.drivefs.shortcuts.sheets` | Google Workspace | Googleの表計算入口 |
| Google Slides | `com.google.drivefs.shortcuts.slides` | Google Workspace | Googleのプレゼンテーション入口 |
| Air | `com.jetbrains.air` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| GoLand | `com.jetbrains.goland` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| IntelliJ IDEA Ultimate | `com.jetbrains.intellij` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| JetBrains Rider | `com.jetbrains.rider` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| JetBrains Toolbox | `com.jetbrains.toolbox` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| PhpStorm | `com.jetbrains.PhpStorm` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| PyCharm Professional Edition | `com.jetbrains.pycharm` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| RustRover | `com.jetbrains.rustrover` | JetBrains | JetBrainsのIDE・開発支援（実測した製品ID） |
| Live2D Cubism Editor 5.3 | `com.live2d.cubism.CECubismEditorApp` | Live2D Cubism | Live2Dモデル・アニメーション制作 |
| Live2D Cubism Viewer (for OW) | `com.live2d.cubism.doc.modeling.ui.viewerForOriginalWorkflow.OWViewerDropFrame` | Live2D Cubism | Live2Dモデルの表示・確認 |
| AirMacユーティリティ | `com.apple.airport.airportutility` | macOSユーティリティ | AirMac機器の設定 |
| Audio MIDI設定 | `com.apple.audio.AudioMIDISetup` | macOSユーティリティ | 音声・MIDIデバイスの設定 |
| Automator | `com.apple.Automator` | macOSユーティリティ | OS操作の自動化 |
| Bluetoothファイル交換 | `com.apple.BluetoothFileExchange` | macOSユーティリティ | Bluetooth接続でのファイル交換 |
| Boot Campアシスタント | `com.apple.bootcampassistant` | macOSユーティリティ | Boot Campの導入管理 |
| ColorSyncユーティリティ | `com.apple.ColorSyncUtility` | macOSユーティリティ | カラープロファイルの管理 |
| DaisyDisk | `com.daisydiskapp.DaisyDiskStandAlone` | macOSユーティリティ | ディスク容量の確認・整理 |
| Digital Color Meter | `com.apple.DigitalColorMeter` | macOSユーティリティ | 画面上の色の測定 |
| Font Book | `com.apple.FontBook` | macOSユーティリティ | フォントの管理 |
| Grapher | `com.apple.grapher` | macOSユーティリティ | 数式・グラフの表示 |
| Mission Control | `com.apple.exposelauncher` | macOSユーティリティ | ウィンドウの管理 |
| RunCat | `com.kyome.RunCat` | macOSユーティリティ | システムの稼働状態の確認 |
| Siri | `com.apple.siri.launcher` | macOSユーティリティ | OSの音声操作 |
| Time Machine | `com.apple.backup.launcher` | macOSユーティリティ | バックアップ管理 |
| VoiceOverユーティリティ | `com.apple.VoiceOverUtility` | macOSユーティリティ | VoiceOverの設定 |
| アクティビティモニタ | `com.apple.ActivityMonitor` | macOSユーティリティ | プロセス・CPU・メモリの診断 |
| イメージキャプチャ | `com.apple.Image_Capture` | macOSユーティリティ | カメラ等からの画像取り込み |
| コンソール | `com.apple.Console` | macOSユーティリティ | システムログの確認 |
| システム情報 | `com.apple.SystemProfiler` | macOSユーティリティ | ハードウェア・システム構成の確認 |
| ショートカット | `com.apple.shortcuts` | macOSユーティリティ | 操作の自動化 |
| スクリプトエディタ | `com.apple.ScriptEditor2` | macOSユーティリティ | AppleScript等の作成・実行 |
| ターミナル | `com.apple.Terminal` | macOSユーティリティ | OS標準のコマンド実行 |
| ディスクユーティリティ | `com.apple.DiskUtility` | macOSユーティリティ | ディスクの管理・診断 |
| パスワード | `com.apple.Passwords` | macOSユーティリティ | 認証情報の管理 |
| ヒント | `com.apple.helpviewer` | macOSユーティリティ | OSのヘルプ |
| プリントセンター | `com.apple.printcenter` | macOSユーティリティ | プリンター・印刷ジョブの管理 |
| 移行アシスタント | `com.apple.MigrateAssistant` | macOSユーティリティ | Macへのデータ移行 |
| QuickTime Player | `com.apple.QuickTimePlayerX` | Macアクセサリ | Mac標準のメディア再生・収録 |
| スクリーンショット | `com.apple.screenshot.launcher` | Macアクセサリ | Mac標準の画面キャプチャ |
| スティッキーズ | `com.apple.Stickies` | Macアクセサリ | Mac標準の付箋メモ |
| テキストエディット | `com.apple.TextEdit` | Macアクセサリ | Mac標準のテキスト編集 |
| ボイスメモ | `com.apple.VoiceMemos` | Macアクセサリ | Mac標準の音声メモ |
| 辞書 | `com.apple.Dictionary` | Macアクセサリ | Mac標準の辞書検索 |
| BetterTouchTool | `com.hegenberg.BetterTouchTool` | Macカスタマイズ | 入力・ジェスチャーのカスタマイズ |
| LauncherTurbo | `com.turbo.LauncherTurbo` | Macカスタマイズ | アプリランチャー |
| Launchpad | `com.apple.launchpad.launcher` | Macカスタマイズ | アプリランチャー |
| Launchpad | `com.kamakiri1192.launchpad` | Macカスタマイズ | アプリランチャー |
| LaunchpadPlus | `waikiki.program.Launchpad` | Macカスタマイズ | アプリランチャー |
| NotchNook | `lo.cafe.NotchNook` | Macカスタマイズ | ノッチ周辺の操作カスタマイズ |
| ScreenNote | `com.kyome.ScreenNote` | Macカスタマイズ | 画面への注釈 |
| ScreenPointer | `com.kyome.ScreenPointer` | Macカスタマイズ | 画面上のポインター強調 |
| Scroll Reverser | `com.pilotmoon.scroll-reverser` | Macカスタマイズ | スクロール方向のカスタマイズ |
| c4dpy | `net.maxon.python` | Maxon | Cinema 4DのPython実行環境 |
| Cinema 4D | `net.maxon.cinema4d` | Maxon | 3D制作 |
| Cinema 4D Team Render Client | `net.maxon.cinema4dclient` | Maxon | Cinema 4Dの分散レンダリング参加 |
| Cinema 4D Team Render Server | `net.maxon.cinema4dserver` | Maxon | Cinema 4Dの分散レンダリング管理 |
| Commandline | `net.maxon.commandline` | Maxon | Cinema 4Dのコマンドライン実行 |
| uninstall | `net.maxon.cinema4d.installer.uninstall` | Maxon | Cinema 4Dの削除管理 |
| Acrobat | `com.adobe.Acrobat.Pro` | PDF | PDF閲覧・編集 |
| Distiller | `com.adobe.distiller` | PDF | PDFの生成 |
| プレビュー | `com.apple.Preview` | PDF | PDF・画像の閲覧と注釈 |
| Desktop Mate | `steam:3301060` | Steamライブラリ | Steamライブラリから検出したゲーム・アプリ |
| Sid Meier's Civilization VI | `steam:289070` | Steamライブラリ | Steamが生成したアプリショートカット（起動先IDを確認） |
| Cubase LE AI Elements 13 | `com.steinberg.cubasesoft13` | Steinberg | DAWでの作曲・録音（版・エディションは同製品） |
| eLicenser Control Center | `com.Syncrosoft.LCC` | Steinberg | Steinberg製品の旧ライセンス管理 |
| Groove Agent SE | `com.steinberg.vst3.GrooveAgentSEStandalone` | Steinberg | 単体起動するドラム音源 |
| HALion Sonic | `com.steinberg.vst3.HALionSonicStandalone` | Steinberg | 単体起動する音源 |
| Steinberg Activation Manager | `com.steinberg.SteinbergActivationManager` | Steinberg | Steinberg製品の認証 |
| Steinberg Download Assistant | `net.steinberg.elicenser.SteinbergDownloadAssistant` | Steinberg | Steinberg製品の導入 |
| Steinberg Library Manager | `com.steinberg.HALionLibraryManager` | Steinberg | Steinberg音源ライブラリの管理 |
| Uninstall Steinberg Download Assistant | `net.steinberg.eLicenser.installer.SteinbergDownloadAssistant.uninstall` | Steinberg | Steinberg Download Assistantの削除管理 |
| Unity | `com.unity3d.UnityEditor5.x` | Unity | Unityでの制作・開発 |
| Unity Bug Reporter | `com.unity3d.bugreporter` | Unity | Unityの不具合報告 |
| Unity Hub | `com.unity3d.unityhub` | Unity | Unity環境の導入・プロジェクト管理 |
| VOCALOID Authorizer | `com.yamaha.VOCALOID-Authorizer` | VOCALOID | VOCALOIDの認証 |
| VOCALOID Authorizer Uninstaller | `com.apple.ScriptEditor.id.VOCALOID_Authorizer-Uninstaller` | VOCALOID | VOCALOID認証ツールの削除管理 |
| VOCALOID Uninstaller | `com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | VOCALOID | VOCALOIDの削除管理 |
| VOCALOID4 API Uninstaller | `com.apple.ScriptEditor.id.VOCALOID4-API-Uninstaller` | VOCALOID | VOCALOID APIの削除管理 |
| VOCALOID6 Editor | `com.yamaha.VOCALOID6-Editor` | VOCALOID | VOCALOIDでの歌声編集 |
| NTE | `com.hottagames.nte` | ゲーム | ゲーム |
| チェス | `com.apple.Chess` | ゲーム | チェス |
| トリッカル・もちもちほっペ大作戦 | `com.bilibili.trickcal` | ゲーム | ゲーム |
| Cavalry | `com.scenegroup.cavalry` | デザイン | モーショングラフィックス制作 |
| Figma | `com.figma.Desktop` | デザイン | UI・グラフィックデザイン |
| Pencil | `dev.pencil.desktop` | デザイン | UIデザイン制作 |
| iPhoneミラーリング | `com.apple.ScreenContinuity` | デバイス連携 | iPhone画面の操作 |
| ホーム | `com.apple.Home` | デバイス連携 | スマートホームの操作 |
| 探す | `com.apple.findmy` | デバイス連携 | デバイス等の位置確認 |
| FileZilla | `org.filezilla-project.filezilla` | ファイル転送・同期 | FTP等のファイル転送 |
| Google Drive | `com.google.drivefs` | ファイル転送・同期 | クラウドファイル同期 |
| Mountain Duck | `io.mountainduck` | ファイル転送・同期 | リモートストレージのマウント |
| Nextcloud | `com.nextcloud.desktopclient` | ファイル転送・同期 | クラウドファイル同期 |
| RemoteDrive | `org.filezilla-project.remotedrive` | ファイル転送・同期 | リモートストレージへの接続 |
| TV | `com.apple.TV` | メディア再生 | 動画視聴 |
| VLC | `org.videolan.vlc` | メディア再生 | 動画・音声再生 |
| YouTube Music | `com.github.th-ch.youtube-music` | メディア再生 | 音楽再生 |
| ブック | `com.apple.iBooksX` | メディア再生 | 電子書籍 |
| ポッドキャスト | `com.apple.podcasts` | メディア再生 | ポッドキャスト再生 |
| ミュージック | `com.apple.Music` | メディア再生 | 音楽再生 |
| Goodnotes | `com.goodnotesapp.x` | メモ・予定・作業 | ノート作成 |
| Koucha | `com.hakocat.Koucha` | メモ・予定・作業 | Safariタブの保存・作業整理（同梱UIとSafari拡張を確認） |
| Notion | `notion.id` | メモ・予定・作業 | ノート・文書・作業管理 |
| Notion Web Clipper | `notion.id.NotionSafariExtension` | メモ・予定・作業 | NotionへのWebクリップ |
| カレンダー | `com.apple.iCal` | メモ・予定・作業 | 予定管理 |
| チアプロ - 集中・継続を応援 | `com.gateboxlab.cheerpro` | メモ・予定・作業 | 集中・継続の作業支援 |
| フリーボード | `com.apple.freeform` | メモ・予定・作業 | ホワイトボード・アイデア整理 |
| メモ | `com.apple.Notes` | メモ・予定・作業 | メモ作成 |
| リマインダー | `com.apple.reminders` | メモ・予定・作業 | タスク管理 |
| 時計 | `com.apple.clock` | メモ・予定・作業 | 時計・アラーム・タイマー |
| Barrier | `barrier` | リモートデスクトップ | ネットワーク経由のキーボード・マウス共有 |
| Chrome Remote Desktop Host Uninstaller | `com.google.chromeremotedesktop.me2me-host-uninstaller` | リモートデスクトップ | Chrome Remote Desktopホストの削除管理 |
| 画面共有 | `com.apple.ScreenSharing` | リモートデスクトップ | Macの画面への遠隔接続 |
| Parallels Desktop | `com.parallels.desktop.console` | 仮想マシン | 仮想マシンの実行 |
| VMware Fusion | `com.vmware.fusion` | 仮想マシン | 仮想マシンの実行 |
| ComfyUI | `com.todesktop.241012ess7yxs0e` | 写真・画像 | 画像生成ワークフロー |
| Photo Booth | `com.apple.PhotoBooth` | 写真・画像 | カメラ撮影・画像エフェクト |
| Playground | `com.apple.GenerativePlaygroundApp` | 写真・画像 | 画像生成 |
| アニメ画像に変換する君 | `media.ics.AnimationImageConverter` | 写真・画像 | アニメーション画像への変換 |
| 写真 | `com.apple.Photos` | 写真・画像 | 写真の管理・編集 |
| iMovie | `com.apple.iMovieApp` | 映像・配信・演出 | 動画編集 |
| Lightkey | `de.monospc.Lightkey` | 映像・配信・演出 | DMX照明・ライブ演出制御 |
| OBS Studio | `com.obsproject.obs-studio` | 映像・配信・演出 | 映像配信・録画 |
| obsctl | `com.wails.obsctl` | 映像・配信・演出 | OBS接続・配信操作（実機のgoobs依存と同梱UIを確認） |
| News | `com.apple.news` | 生活情報 | ニュース閲覧 |
| マップ | `com.apple.Maps` | 生活情報 | 地図・経路検索 |
| 天気 | `com.apple.weather` | 生活情報 | 天気の確認 |
| 株価 | `com.apple.stocks` | 生活情報 | 株価情報の確認 |
| App Store | `com.apple.AppStore` | 直接表示 | アプリ入手・更新の入口なので直接表示 |
| Google Chrome | `com.google.Chrome` | 直接表示 | 日常のブラウザ入口なので直接表示 |
| Safari | `com.apple.Safari` | 直接表示 | 日常のブラウザ入口なので直接表示 |
| Steam | `com.valvesoftware.steam` | 直接表示 | ゲームライブラリの入口なので直接表示 |
| システム設定 | `com.apple.systempreferences` | 直接表示 | OS設定の入口なので直接表示 |
| 計算機 | `com.apple.calculator` | 直接表示 | すぐ使う計算ツールなので直接表示 |
| Discord | `com.hnc.Discord` | 連絡・通話 | コミュニティ・通話 |
| FaceTime | `com.apple.FaceTime` | 連絡・通話 | ビデオ通話 |
| LINE | `jp.naver.line.mac` | 連絡・通話 | メッセージ・通話 |
| X | `com.atebits.Tweetie2` | 連絡・通話 | SNS |
| zoom.us | `us.zoom.xos` | 連絡・通話 | オンライン会議 |
| メッセージ | `com.apple.MobileSMS` | 連絡・通話 | メッセージ送受信 |
| メール | `com.apple.mail` | 連絡・通話 | メール送受信 |
| 連絡先 | `com.apple.AddressBook` | 連絡・通話 | 連絡先管理 |
| Antigravity | `com.google.antigravity` | 開発ツール | AI開発環境 |
| Code | `com.microsoft.VSCode` | 開発ツール | コードエディタ |
| Cursor | `com.todesktop.230313mzl4w4u92` | 開発ツール | AIコードエディタ |
| Fork | `com.DanPristupov.Fork` | 開発ツール | Gitクライアント |
| iTerm2 | `com.googlecode.iterm2` | 開発ツール | 開発用ターミナル |
| iTermBrowserPlugin | `com.googlecode.iterm2.iTermBrowserPlugin` | 開発ツール | iTermのブラウザ補助機能 |
| Kiro | `dev.kiro.desktop` | 開発ツール | AI開発環境 |
| OpenCode | `ai.opencode.desktop` | 開発ツール | コーディングエージェント |
| RapidAPI | `com.luckymarmot.Paw` | 開発ツール | HTTP APIクライアント |
| Remote Codetrol Server | `com.scicotherapylabs.RemoteCodetrolServer` | 開発ツール | コーディングエージェントの遠隔開発サーバー |
| TablePlus | `com.tinyapp.TablePlus` | 開発ツール | データベース管理クライアント |
| Visual Studio | `com.microsoft.visual-studio` | 開発ツール | 統合開発環境 |
| Xcode | `com.apple.dt.Xcode` | 開発ツール | 統合開発環境 |
| ZCode | `dev.zcode.app` | 開発ツール | コード開発ツール |
| Crypton Product Manager | `jp.co.crypton.CryptonProductManager` | 音声・歌声合成 | 音源・Piapro関連製品の管理 |
| Piapro Studio NT | `jp.co.crypton.PiaproStudio` | 音声・歌声合成 | 歌声合成の編集 |
| sbv2-gui | `jp.tuna2134.sbv2-gui` | 音声・歌声合成 | Style-Bert-VITS2の音声合成 |
| Synthesizer V Studio 2 Pro | `com.dreamtonics.svstudio2.pro` | 音声・歌声合成 | 歌声合成の編集（同製品の別版） |
| Synthesizer V Studio Basic | `com.dreamtonics.svstudio.basic` | 音声・歌声合成 | 歌声合成の編集（同製品の別版） |
| Synthesizer V Studio Pro | `com.dreamtonics.svstudio.pro` | 音声・歌声合成 | 歌声合成の編集（同製品の別版） |
| Capo | `com.supermegaultragroovy.capo3.mac` | 音楽制作 | 音楽の耳コピ・練習支援 |
| GarageBand | `com.apple.garageband10` | 音楽制作 | 作曲・録音 |
| MIDI Monitor | `com.snoize.MIDIMonitor` | 音楽制作 | MIDI信号の確認 |
| TouchOSC | `net.hexler.lex` | 音楽制作 | MIDI・OSCの制作コントローラー |

## 表示対象に統合・除外した53個のアプリ束

すべて以下に記録する。重複は既存の識別子統合、バックグラウンド専用は既存のOSフラグ判定による。ParsecのGUI用途は確認できるが、この端末のbundleは `LSBackgroundOnly=true` のため通常スキャナーでは表示されない。

| アプリ束の場所 | 識別子 | 扱い・理由 |
| --- | --- | --- |
| `/Applications/Adobe After Effects 2024/Adobe After Effects Render Engine 2024.app` | `mac:com.adobe.AfterEffectsRenderEngine` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe After Effects 2024/Plug-ins/Effects/mochaAE/MochaAE.bundle/Contents/MacOS/mochaui/Mocha AE.app` | `com.borisfx.mocha4ae_adobe` | プラグイン内部の深い旧版。表示対象のMocha AEと同じID → Adobe Creative Cloud |
| `/Applications/Adobe After Effects 2025/Adobe After Effects 2025.app` | `mac:com.adobe.AfterEffects.application` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe After Effects 2025/Adobe After Effects Render Engine 2025.app` | `mac:com.adobe.AfterEffectsRenderEngine` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe After Effects 2025/Plug-ins/Effects/mochaAE/MochaAE.bundle/Contents/MacOS/mochaui/Mocha AE.app` | `com.borisfx.mocha4ae_adobe` | プラグイン内部の深い旧版。表示対象のMocha AEと同じID → Adobe Creative Cloud |
| `/Applications/Adobe Audition 2024/Adobe Audition 2024.app` | `mac:com.adobe.Audition` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Audition 2025/Adobe Audition 2025.app` | `mac:com.adobe.Audition` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Bridge 2026/Adobe Bridge 2026.app` | `mac:com.adobe.bridge` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2024/Adobe Illustrator.app` | `mac:com.adobe.illustrator` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2024/Scripting.localized/Sample Scripts.localized/AppleScript.localized/Calendar.localized/Make Calendar.app` | `mac:com.apple.ScriptEditor.id.Make-Calendar` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2024/Scripting.localized/Sample Scripts.localized/AppleScript.localized/Contact Sheet Demo.localized/Contact Sheets.app` | `mac:com.apple.ScriptEditor.id.Contact-Sheets` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2024/Scripting.localized/Sample Scripts.localized/AppleScript.localized/Web Gallery.localized/Web Gallery.app` | `mac:com.apple.ScriptEditor.id.Web-Gallery` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2025/Adobe Illustrator.app` | `mac:com.adobe.illustrator` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2025/Scripting.localized/Sample Scripts.localized/AppleScript.localized/Calendar.localized/Make Calendar.app` | `mac:com.apple.ScriptEditor.id.Make-Calendar` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2025/Scripting.localized/Sample Scripts.localized/AppleScript.localized/Contact Sheet Demo.localized/Contact Sheets.app` | `mac:com.apple.ScriptEditor.id.Contact-Sheets` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Illustrator 2025/Scripting.localized/Sample Scripts.localized/AppleScript.localized/Web Gallery.localized/Web Gallery.app` | `mac:com.apple.ScriptEditor.id.Web-Gallery` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe InDesign 2024/Adobe InDesign 2024.app` | `mac:com.adobe.InDesign` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe InDesign 2026/Adobe InDesign 2026.app` | `mac:com.adobe.InDesign` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Photoshop 2024/Adobe Photoshop 2024.app` | `mac:com.adobe.Photoshop` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/Adobe Photoshop 2025/Adobe Photoshop 2025.app` | `mac:com.adobe.Photoshop` | 同じ識別子の表示対象へ統合 → Adobe Creative Cloud |
| `/Applications/BuhoLaunchpad.app` | `com.drbuho.BuhoLaunchpad.Launcher` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/Maxon Cinema 4D 2024/Cinema 4D Team Render Client.app` | `mac:net.maxon.cinema4dclient` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2024/Cinema 4D Team Render Server.app` | `mac:net.maxon.cinema4dserver` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2024/Cinema 4D.app` | `mac:net.maxon.cinema4d` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2024/Commandline.app` | `mac:net.maxon.commandline` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2024/c4dpy.app` | `mac:net.maxon.python` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2025/Cinema 4D Team Render Client.app` | `mac:net.maxon.cinema4dclient` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2025/Cinema 4D Team Render Server.app` | `mac:net.maxon.cinema4dserver` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2025/Cinema 4D.app` | `mac:net.maxon.cinema4d` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2025/Commandline.app` | `mac:net.maxon.commandline` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2025/c4dpy.app` | `mac:net.maxon.python` | 同じ識別子の表示対象へ統合 → Maxon |
| `/Applications/Maxon Cinema 4D 2025/resource/modules/c4d_assetbrowser/mxai/osx/MXAI_ImageSense.app` | `net.maxon.MXAI_ImageSense` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/Maxon Cinema 4D 2026/resource/modules/c4d_assetbrowser/mxai/osx/MXAI_ImageSense.app` | `net.maxon.MXAI_ImageSense` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/OBS.app` | `mac:com.obsproject.obs-studio` | 同じ識別子の表示対象へ統合 → 映像・配信・演出 |
| `/Applications/Parsec.app` | `tv.parsec.www` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/Unity/Hub/Editor/2022.3.19f1/Unity/Unity Bug Reporter.app` | `mac:com.unity3d.bugreporter` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/2022.3.19f1/Unity/Unity.app` | `mac:com.unity3d.UnityEditor5.x` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/2022.3.19f1/Unity Bug Reporter.app` | `mac:com.unity3d.bugreporter` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/2022.3.19f1/Unity.app` | `mac:com.unity3d.UnityEditor5.x` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/2022.3.22f1/Unity Bug Reporter.app` | `mac:com.unity3d.bugreporter` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/2022.3.22f1/Unity.app` | `mac:com.unity3d.UnityEditor5.x` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/2023.2.16f1/Unity Bug Reporter.app` | `mac:com.unity3d.bugreporter` | 同じ識別子の表示対象へ統合 → Unity |
| `/Applications/Unity/Hub/Editor/6000.0.54f1/Unity.app` | `com.unity3d.UnityEditor5.x` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/Utilities/Adobe Creative Cloud/ACC/Creative Cloud Helper.app` | `com.adobe.ccd.helper` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/Utilities/Adobe Creative Cloud Experience/CCXProcess/CCXProcess.app` | `com.adobe.CCXProcess` | バックグラウンド専用フラグにより表示対象外 |
| `/Applications/VOCALOID/Uninstaller/VOCALOID4/KAITO_SP Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID/Uninstaller/VOCALOID4/LEN_SP Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID/Uninstaller/VOCALOID4/LUKA_SP Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID/Uninstaller/VOCALOID4/MEIKO_SP Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID/Uninstaller/VOCALOID4/MIKU_SP Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID/Uninstaller/VOCALOID4/RIN_SP Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID6/Uninstaller/VOCALOID6 Editor Uninstaller.app` | `mac:com.apple.ScriptEditor.id.VOCALOID-Uninstaller` | 同じ識別子の表示対象へ統合 → VOCALOID |
| `/Applications/VOCALOID6/VOCALOID Authorizer.app` | `mac:com.yamaha.VOCALOID-Authorizer` | 同じ識別子の表示対象へ統合 → VOCALOID |

## 誤分類を避ける確認

- ComfyUIは画像生成、YouTube Musicはメディア再生、3teneとWebcam Motion Receiverはアバター制作、Zoomは通話、Remote Codetrolは開発支援。`LSApplicationCategoryType` にはこれらと異なる宣言があるため、OSカテゴリだけで判定しない。
- Kouchaは同梱UIのタブ保存メッセージとSafari拡張で用途を確認。obsctlは同梱UIのOBS接続と `goobs` 依存を確認。同名の別プロジェクトの説明で判断しない。
- メーカー接頭辞・表示名だけでは分類しない。既知のBundle IDの完全一致か、既存の数値版ルールを使う。未知のアプリは直接表示に残す。
- IllustratorのAppleScriptサンプルは汎用のScript Editor生成IDを使うため、IDに加えて `Adobe Illustrator` または4桁年付きの所有ディレクトリを照合する。個人の同名スクリプトは取り込まない。
- Creative Cloudの導入・診断・同期・削除、iTermブラウザ補助、Notion Clipper、CLIP STUDIO内部モジュール、Cinema 4DのPython・削除ツールは、所有製品と同じ製品キー。これらだけで2製品条件を満たさない。
- Synthesizer V Basic/Pro/2 Proは同じ製品キー。版違いだけではフォルダを作らない。
- SteamのNUL付きXMLは、生成された `run.sh` が厳密な `open steam://run/<数字>` である場合だけ復旧する。スクリプトは実行しない。任意の破損データや追加コマンドはスキャンエラーのまま。

用途確認で参照した一次資料: [Remote Codetrol](https://remotecodetrol.ai/)、[Lightkey](https://www.lightkeyapp.com/en)、[Looking Glass Bridge](https://looking-glass.helpscoutdocs.com/article/318-where-can-i-download-looking-glass-bridge-and-looking-glass-studio)、[SBV2 GUI](https://github.com/tuna2134/sbv2-gui)、[LauncherTurbo](https://github.com/Turbo1123/LauncherTurbo)。識別子は実機のメタデータで確認した。

## 再実行

```sh
cargo run --locked --example macos_initial_layout_audit > target/macos-layout-full-audit.json
```

読取専用。通常の全providerと同じ217件の配置案に加え、全アプリ束の検出・重複・除外理由をJSONに出力する。保存DBや実際のレイアウトへ適用しない。追加アプリがある端末では件数が変わり、未知のIDは `unresolved` として報告する。
