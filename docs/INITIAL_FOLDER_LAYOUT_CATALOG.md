# 初期フォルダ配置の同梱カタログ

仕様の19候補と追加15候補（Git、Unity、Live2D Cubism、Razer、アクセシビリティ、VRツール、VR Games、リモートデスクトップ、Windows Dev tools、OMEN、Windowsアクセサリ、Macアクセサリ、Google Workspace、CLIP STUDIO、VOCALOID）に対応する。従来のWindowsツールはPC管理へ改めた。定義は `src/domain/initial_layout/catalog.rs`、`catalog/products.rs`、`catalog/windows.rs`、`catalog/macos.rs`。カタログ版は6。
確認日: Windowsは2026-09-25、Macの追加対応は2026-09-30。調査資料は開発時に参照し、実行時にはネットワークへアクセスしない。
Macの最新の分類範囲・実機結果・検証方法は[macOSの初期フォルダ分類](MACOS_INITIAL_FOLDER_LAYOUT.md)を参照する。以下の旧版確認資料も識別子の根拠として維持する。

## 共通の判定

- Windowsでは解決した実行ファイルとインストール先、または実行ファイルとProductName・CompanyNameを組み合わせる。表示名の変更や翻訳は判定に影響しない。
- 通常のWindowsアプリは取得済みの空の引数を条件とする。LibreOfficeとVisual Studioは起動機能を表す引数を個別に照合する。
- macOSではCFBundleIdentifierを照合する。版を含むIDは固定の製品部分と数値の版部分を分け、同じ製品キーにまとめる。メーカーの接頭辞だけでは判定しない。
- 同じ製品の複数版、重複ショートカットだけではフォルダを作らない。異なる製品・付属機能と既知の起動対象が、それぞれ2種類以上あるときだけ作る。
- Steam manifest由来の項目は、明示したVR用App IDだけをVRツール／VR Gamesへ分類し、それ以外はSteamライブラリへ分類する。JetBrains Toolbox、Creative Cloud管理アプリ、Unity Hubと製品付属ツールも明示的に含める。無関係な更新プログラム、機器ドライバ、プラグインは混ぜない。

## 19候補の対象

| ID | Windows | macOS | 境界 |
| --- | --- | --- | --- |
| `windows-tools`（PC管理） | ディスククリーンアップ、ドライブ最適化、iSCSI、メモリ診断、ODBC、回復ドライブ、レジストリエディタ、パフォーマンス/リソースモニタ、システム構成・情報、管理コンソール、Windowsセキュリティ、バックアップ、問い合わせ、フィードバック、NVIDIA設定、Ut Video/x264設定 | — | OS配下の限定exe・mscと引数、既知のShell ID。単なる表示名やrundll32/mmcだけで判定しない。設定・電卓・エクスプローラー・Storeは直接表示 |
| `macos-utilities` | — | アクティビティモニタ、コンソール、ディスクユーティリティ | `com.apple.ActivityMonitor` / `Console` / `DiskUtility`。Apple製全体をまとめない |
| `microsoft-office` | `Microsoft Office`配下の`WINWORD.exe`、`EXCEL.EXE`、`POWERPNT.EXE` | `com.microsoft.Word` / `Excel` / `Powerpoint` | Teams・Edge・OneDriveは外へ残す |
| `apple-documents` | — | Pages、Numbers、Keynote | `com.apple.iWork.*`の旧IDと`com.apple.Pages` / `Numbers` / `Keynote`。新旧同製品を別製品として数えない |
| `libreoffice` | `soffice.exe`の`--writer` / `--calc` / `--impress` / `--draw` / `--math` / `--base`、および`swriter` / `scalc` / `simpress` / `sdraw` / `smath` / `sbase`の個別exe | `org.libreoffice.script` | 同じコンポーネントの起動方法違いを別製品として数えない。Macの単一アプリからWriter等を合成しない |
| `adobe-creative-cloud` | Photoshop、Illustrator、After Effects、Audition、Media Encoder、Premiere Pro/Premiere | 同じ6製品 | Creative CloudクライアントもWindowsで含める。Reader・更新用ヘルパーは外へ残す |
| `affinity-legacy` | `Affinity`配下のDesigner、Photo、Publisherのexe | Designer / Photo / Publisherのv1・v2の独立bundle | 統合版を3アプリへ分解しない。v1とv2だけでは別製品にならない |
| `coreldraw-suite` | `Corel`配下の`CorelDRW.exe`、`CorelPP.exe` | CorelDRAW、PHOTO-PAINT | Painterやインストーラを混ぜない。Store版DRAWと版付きsuite IDに対応 |
| `cyberlink-director-suite` | `CyberLink`配下のPowerDirector、PhotoDirector、AudioDirector、ColorDirector | PowerDirector、PhotoDirector | MacにWindows専用製品のルールを適用しない |
| `final-cut-suite` | — | Final Cut Pro、Motion、Compressor | 買い切り版とCreator StudioのIDを同一製品にまとめる |
| `logic-suite` | — | Logic Pro、MainStage | GarageBandや音源ライブラリは混ぜない |
| `blackmagic-creative` | `Blackmagic Design`配下の`Resolve.exe`、`Fusion.exe` | DaVinci Resolve、単体Fusion | Resolve内のFusionページや機器ドライバを独立アプリとして扱わない |
| `maxon-creative` | Cinema 4D、ZBrush（Maxon/Pixologicの配置） | Cinema 4D、ZBrush | WindowsはCommandline、Team Render Client/Serverも付属機能として含める。プラグインは加えない |
| `steinberg-creative` | `Steinberg`配下のCubase、Dorico、WaveLab | Cubase、Dorico、WaveLab | Download Assistant、Activation Manager、VSTは混ぜない |
| `native-instruments-standalone` | `Native Instruments`配下のKontakt、Reaktorのexe | Kontakt、Reaktorの単体bundle | Native Access、音源ライブラリ、VST/AUは混ぜない |
| `jetbrains-ides` | IntelliJ IDEA、PyCharm、WebStorm、PhpStorm、CLion、GoLand、Rider、RubyMine、DataGrip | 同じ9製品 | Windowsはexe・ProductName・JetBrainsのCompanyNameを照合。Toolboxも同じフォルダに含める |
| `autodesk-creative` | `Autodesk`配下のAutoCAD、Maya、3ds Max | AutoCAD、Maya | 3ds MaxのMacルールは作らない。ライセンス管理・更新ツールは混ぜない |
| `visual-studio-tools` | VCの4種類の環境設定プロンプト、Developer Command Prompt、Developer PowerShell | — | Microsoftのcmd/PowerShellとVS配下のスクリプト・モジュールを照合。IDE・VS Codeは外へ残す |
| `steam-library` | manifest由来のゲーム・制作アプリ | 同左 | 表示名に「Steam」が含まれるだけのアプリは判定しない。Steamクライアントは外へ残す |

## 識別情報の確認資料

### Windows実機

この開発端末のStart Menuショートカット、解決先exeのProductName/CompanyName、引数を読み取った。
アプリの起動・設定変更は行っていない。

- Adobe: After Effects 2025/2026、Audition 2025/2026、Illustrator 2026、Media Encoder 2025/2026、Photoshop 2026、Premiere Pro 2025、Premiere 2026。10項目が6製品として1フォルダになる。
- AdobeのCompanyNameは `Adobe`、`Adobe Inc.`、`Adobe Systems Incorporated`。ProductNameの末尾の4桁年だけを除き、exeとの組を照合する。
- Cinema 4D 2025/2026、Commandline、Team Render Client/Server。関連ツールも含めてMaxonフォルダにまとめる。同じCinema 4Dの複数版だけでは作らない。
- IntelliJ IDEAの`idea64.exe`、`IntelliJ IDEA`、`JetBrains s.r.o.`。
- Visual Studio 18 Build Tools: `vcvars64.bat`、`vcvars32.bat`、`vcvarsamd64_x86.bat`、`vcvarsx86_amd64.bat`、`Common7/Tools/VsDevCmd.bat`、Developer PowerShellの`Import-Module`と`Enter-VsDevShell`の完全な引数構造。
- Windowsツール: `cleanmgr`、`dfrgui`、`iscsicpl`、`MdSched`、`odbcad32`、`RecoveryDrive`、`regedit`、`perfmon`、`msconfig`、`msinfo32`。SystemRoot直下/System32/SysWOW64に限定する。

### 製品別資料

識別子は資料中の実際のパス、アプリのInfo.plist、配布レシピ、製品自身が生成した診断記録から確認する。診断記録はその版の識別情報の根拠として使い、動作保証とは扱わない。

- Office: [MicrosoftのWindows実行ファイル資料](https://learn.microsoft.com/en-us/previous-versions/office/troubleshoot/office-developer/use-visual-c-automate-run-program-instance)、[Macのbundle ID](https://learn.microsoft.com/en-us/office/dev/add-ins/testing/sideload-an-office-add-in-on-mac)。
- LibreOffice: [公式起動パラメータ](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html)、[Windowsの個別ランチャー定義](https://github.com/LibreOffice/core/blob/master/desktop/Module_desktop.mk)。
- Visual Studio: [Microsoftの開発者シェル資料](https://learn.microsoft.com/en-us/dotnet/framework/tools/developer-command-prompt-for-vs)。PowerShellは実機で取得したショートカットの構造を照合する。
- WindowsのAdobe、Affinity Designer/Photo、CorelDRAW/PHOTO-PAINT、Cinema 4D、Resolve、AutoCAD/Maya/3ds Max: [NVIDIAの各製品の実行ファイル・既定パス資料](https://nvidia.custhelp.com/app/answers/detail/a_id/5188)。
- Affinity Publisher: [製品フォーラムの実行ファイル診断](https://forum.affinity.serif.com/index.php?/topic/162684-why-are-there-so-many-crash-dump-files-in-affinity-data/)。Mac v1は実測Info.plist（[Designer](https://doesitarm.com/app/affinity-designer)、[Photo](https://doesitarm.com/app/affinity-photo)、[Publisher](https://doesitarm.com/app/affinity-publisher)）、v2はHomebrewの配布レシピ（[Designer](https://formulae.brew.sh/api/cask/affinity-designer.json)、[Photo](https://formulae.brew.sh/api/cask/affinity-photo.json)、[Publisher](https://formulae.brew.sh/api/cask/affinity-publisher.json)）。
- Mac Adobe: 実測Info.plist（[Photoshop](https://doesitarm.com/app/photoshop)、[Illustrator](https://doesitarm.com/app/illustrator)、[After Effects](https://doesitarm.com/app/after-effects)、[Premiere Pro](https://doesitarm.com/app/premiere-pro)）、[Auditionの導入記録](https://groups.google.com/g/munki-dev/c/B7zqZgh8FWE)、[Media Encoderの製品診断](https://community.adobe.com/questions-506/media-encoder-v14-3-crash-at-launch-998741)。
  Auditionは[現行形式の実測Info.plist](https://doesitarm.com/app/adobe-audition)の`com.adobe.Audition`と、旧形式の数値版付きIDを同じ製品として扱う。
- Mac Corel: 実測Info.plist（[CorelDRAW](https://doesitarm.com/app/coreldraw/)、[PHOTO-PAINT](https://doesitarm.com/app/corel-photo-paint-2021)）。`com.corel.coreldrawsuite.<年>.coreldraw` / `.photopaint` とStoreの `com.corel.CorelDRAW`。
- CyberLink: [PhotoDirectorの実行ファイル資料](https://support.cyberlink.com/faq/photodirector-4-exe-stopped-working-error)、[AudioDirectorの製品サポート](https://www.cyberlink.com/support/search-product-all-faq.do?prodId=252&prodVerId=-1)。Macの2製品は下記App Store配布情報。
- Blackmagic: [ResolveのMac識別子](https://forum.blackmagicdesign.com/viewtopic.php?f=21&t=109197)、[単体Fusionの製品診断](https://forum.blackmagicdesign.com/viewtopic.php?f=22&t=213969)。
- Maxon: [Cinema 4Dの実測Info.plist](https://doesitarm.com/app/cinema-4d)、[ZBrushのMac製品診断](https://www.zbrushcentral.com/t/2021-6-4-mac-os-big-sur/403252)、[公式ZBrush起動パス](https://help.maxon.net/zbr/en-us/Content/html/reference-guide/zscript/zscript-python.html)。
- Steinberg: [Cubaseの実行パス](https://forums.steinberg.net/t/help-os-crashes-dropouts-and-dmp-files/1027961)、[CubaseのMac識別子](https://forums.steinberg.net/t/crash-lock-up-when-return-to-left-locator/758439)、[Doricoの配布レシピ](https://formulae.brew.sh/api/cask/dorico.json)、[WaveLab Pro 12の実行パス](https://forums.steinberg.net/t/support-non-existent/913634?page=2)、[WaveLab 9.5の公式exe資料](https://helpcenter.steinberg.de/hc/en-us/articles/115001257310-High-DPI-HiDPI-UHD-4K-5K-Retina-support-in-WaveLab-9-5)、[WaveLab 13の製品診断](https://forums.steinberg.net/t/wavelab-13-crashes-disables-sonarworks-soundid-reference/1034219)。Mac WaveLabは製品診断中の `net.steinberg.WaveLab-13` を元に数値版を照合。
- Native Instruments: [Kontakt 8の実行パス](https://community.native-instruments.com/discussion/48419/loading-unloading-time-suddenly-increased)、[Kontakt 8のMac製品診断](https://community.native-instruments.com/discussion/comment/195157)、[Kontaktの識別情報](https://community.native-instruments.com/discussion/1149/currently-using-komplete-ultimate-how-do-i-know-that-i-can-safely-migrate-to-macos-monterey)、[Reaktor 6の導入情報](https://community.native-instruments.com/discussion/19940/reaktor-6-player-not-shown-in-finder)。
- JetBrains: 各製品の配布レシピのbundle ID（[IDEA](https://formulae.brew.sh/api/cask/intellij-idea.json)、[PyCharm](https://formulae.brew.sh/api/cask/pycharm.json)、[WebStorm](https://formulae.brew.sh/api/cask/webstorm.json)、[PhpStorm](https://formulae.brew.sh/api/cask/phpstorm.json)、[CLion](https://formulae.brew.sh/api/cask/clion.json)、[GoLand](https://formulae.brew.sh/api/cask/goland.json)、[Rider](https://formulae.brew.sh/api/cask/rider.json)、[RubyMine](https://formulae.brew.sh/api/cask/rubymine.json)、[DataGrip](https://formulae.brew.sh/api/cask/datagrip.json)）。Windowsは各IDEの64bit launcherを限定列挙する。
- Autodesk Mac: [AutoCADの実測Info.plist](https://doesitarm.com/app/autocad)、[Mayaの製品診断](https://forums.autodesk.com/t5/maya-forum/maya-crashing-on-macbook-pro-m2-max/td-p/12133344)。
- macOSユーティリティ: [Activity Monitorの製品診断](https://discussions.apple.com/thread/255701831)、[Consoleの製品診断](https://discussions.apple.com/thread/3734077)。

### Appleの配布情報

Appleの[Search API](https://itunes.apple.com/search?term=Pages&entity=macSoftware&limit=10&country=us)から `trackName`、`bundleId`、`sellerName`、`version` を取得し、配布元を確認した。検索語はPages、Numbers、Keynote、Final Cut Pro、Motion、Compressor、Logic Pro、MainStage、PowerDirector、PhotoDirector、CorelDRAW、DaVinci Resolve。

Final Cut関連は `com.apple.FinalCut` / `FinalCutApp`、`com.apple.motionapp` / `motionappApp`、`com.apple.Compressor` / `CompressorApp`。Logicは `com.apple.logic10`、MainStageは `com.apple.mainstage3`。CyberLinkは `com.cyberlink.powerdirectormac` / `photodirectormac`。

## 検証

`tests/initial_folder_layout.rs` で、30定義すべてのルール存在、OS別製品群の実際の生成・リセット、異なる言語の表示名、重複、複数版だけのケース、ヘルパー、別パス、余計な引数、競合、アプリ集合の保存を検証する。`tests/initial_layout_order.rs` は用途別の優先順、Windowsフォルダの分割、共有ホストと別引数の境界、版の代表選択、並び順だけの取り消しを検証する。

Adobeの10制作アプリとCreative Cloudクライアントは実機スキャンを生成器へ通した読取専用診断と、`qa/initial_folder_layout_adobe_reset.json` のGPU画面テストで確認する。macOSのルールと生成処理はWindows上の純粋関数テストで検証し、Mac実機の表示確認とは区別する。

## 追加候補と端末上の結果

[未分類アプリの確認・追加ルール](INITIAL_FOLDER_LAYOUT_AUDIT.md)に、追加5候補、既存候補の拡張、残りの検討候補を記載する。

## 追加の用途別分類（Windows）

| ID / 表示名（日本語設定） | 対象と識別条件 |
| --- | --- |
| `vr-tools` / VRツール | Meta Horizon Linkの固定Supportパス、SlimeVR Server、Virtual Desktop Streamer、VRChat Creator Companionの専用exeと配置。SteamVR (`250820`)・XSOverlay (`1173510`) はmanifestのApp ID |
| `vr-games` / VR Games | VRChat (`438100`)・Beat Saber (`620980`)・Half-Life: Alyx (`546560`) のSteam App ID。表示名の「VR」では推測しない |
| `remote-desktop` / リモートデスクトップ | Parsec・Paperspaceの専用exeと配置、SystemRoot配下のMicrosoft製mstsc。Chrome Remote DesktopはChrome配置のchrome_proxy.exeとDefaultプロファイル・固定app-idの実機で確認した引数に限定 |
| `windows-dev-tools` / Windows Dev tools | Windows Kits 10のApp Cert Kit、SDKフォルダを開く限定explorer引数、Visual Studio Installerの専用配置、OSのApplication Verifier。WinDbg・Dev Homeは下記AUMID |
| `omen` / OMEN | OMEN Audio Control・OMEN Gaming Hubの下記AUMID |
| `windows-accessories` / Windowsアクセサリ | メモ帳、Snipping Tool、ペイント、クロック、録音、付箋、ターミナル、実行ダイアログは既知のShell ID。従来型のメモ帳・切り取り・ペイント・コマンドプロンプト・文字コード表・タスクマネージャー・PowerShell/ISEはSystemRoot内の限定パス、Microsoftの発行元、既知の起動引数 |

Windows AppsFolder / Get-StartAppsで確認したAUMID（2026-09-25）:

- `AD2F1837.OMENAudioControl_v10z8vjag6ke6!App`
- `AD2F1837.OMENCommandCenter_v10z8vjag6ke6!AD2F1837.OMENCommandCenter`
- `Microsoft.WinDbg_8wekyb3d8bbwe!Microsoft.WinDbg`
- `Microsoft.Windows.DevHome_8wekyb3d8bbwe!App`

Storeアプリは`link_path`の`shell:AppsFolder\` URIからAUMIDを照合し、実行ファイルや引数を取得できなくても起動対象を識別する。
同じAUMIDの複数項目だけではフォルダを作らない。
Steamアプリを非表示にする設定はVR分類にも適用する。異なる製品が1つだけなら平置きを維持する。

実機未導入のVRタイトルは公式掲載でIDを確認した: [Beat Saber](https://store.steampowered.com/app/620980/Beat_Saber/)、[Half-Life: Alyx](https://store.steampowered.com/app/546560/HalfLife_Alyx/)。

## 初期優先順とWindows分割（v5）

並び順の定義は `src/domain/initial_layout/order.rs`、Windowsの識別子は `catalog/windows.rs` にある。初回と設定リセットだけに適用する。保存済みの名前や位置から再分類しない。

- 2026-09-25の実機でShell ID、解決先、引数、ProductName、CompanyNameを確認した。設定・Store・電卓・エクスプローラーは直接前方、Windowsアクセサリは作業帯、PC管理は最後の管理帯とする。
- Chrome/Edgeは専用配置・製品名・発行元、Discord/OBS/Blender/Unreal Editor/VS Codeはexe・製品名・発行元を組み合わせる。いずれも取得済みの空引数を使い、特定ファイルやURLを開くショートカットは前方候補にしない。
- PC管理の.mscはSystemRootのSystem32/SysWOW64にある既知の管理コンソールに限定。mmc.exeはSystem32の既知.mscを指定する引数を照合する。別ディレクトリの同名ファイルは対象外。
- Global ConfigurationはMicrosoftのrundll32.exeからOS配下の `utv_core.dll,GlobalConfigDialog` を指定する実機の形式に限定。x264設定も `x264vfw.dll,Configure` / `x264vfw64.dll,Configure` に限定する。
- NVIDIA Control Panel、Windowsセキュリティ、バックアップ、問い合わせ、フィードバックは実機で確認した完全なShell IDを照合する。
- 同じ製品の複数版がトップレベルに残る場合は、数値のリリース版情報（UEは `++UE5+Release-<版>-CL-<数値>` も含む）で代表を選ぶ。判定できない場合は安定ID順。他の版も一般帯へ残す。

初期優先順の全体像は[配置設計](INITIAL_LAYOUT_ORDER_PROPOSAL.md)を参照する。macOSの表示順は変更しない。
