//! macOS product allowlists, verified against application Info.plist metadata.
//! See docs/MACOS_INITIAL_FOLDER_LAYOUT.md for observed IDs and boundaries.
use super::Rule;

pub(super) fn append_rules(preset: &'static str, rules: &mut Vec<Rule>) {
    if preset == "coreldraw-suite" {
        rules.push(Rule::MacCorelSuite);
    }
    for &(group, bundle, product) in MAC_BUNDLES {
        if group == preset {
            rules.push(Rule::MacBundle { bundle, product });
        }
    }
    for &(group, prefix, product) in MAC_VERSIONED_BUNDLES {
        if group == preset {
            rules.push(Rule::MacVersionedBundle { prefix, product });
        }
    }
    if preset == "adobe-creative-cloud" {
        for bundle in [
            "com.apple.ScriptEditor.id.Contact-Sheets",
            "com.apple.ScriptEditor.id.Make-Calendar",
            "com.apple.ScriptEditor.id.Web-Gallery",
        ] {
            rules.push(Rule::MacBundleInDirectory {
                bundle,
                directory: "Adobe Illustrator",
                product: "illustrator",
            });
        }
    }
}

pub(super) fn keep_top_level(bundle: &str) -> bool {
    matches!(
        bundle,
        "com.apple.AppStore"
            | "com.apple.Safari"
            | "com.google.Chrome"
            | "com.microsoft.edgemac"
            | "com.apple.systempreferences"
            | "com.apple.calculator"
            | "com.valvesoftware.steam"
    )
}

const MAC_BUNDLES: &[(&str, &str, &str)] = &[
    // Full installed-application audit: exact purposes and suite support tools.
    (
        "adobe-creative-cloud",
        "com.adobe.AfterEffectsRenderEngine",
        "after-effects",
    ),
    ("adobe-creative-cloud", "com.adobe.accmac", "creative-cloud"),
    (
        "adobe-creative-cloud",
        "com.adobe.cc.Adobe-Creative-Cloud-Diagnostics",
        "creative-cloud",
    ),
    (
        "adobe-creative-cloud",
        "com.adobe.Creative-Cloud-Desktop-App",
        "creative-cloud",
    ),
    (
        "adobe-creative-cloud",
        "com.adobe.Install",
        "creative-cloud",
    ),
    (
        "adobe-creative-cloud",
        "com.adobe.ACCC.Uninstaller",
        "creative-cloud",
    ),
    (
        "adobe-creative-cloud",
        "com.borisfx.mocha4ae_adobe",
        "after-effects",
    ),
    ("clip-studio", "jp.co.celsys.AggregateMdul", "launcher"),
    ("clip-studio", "jp.co.celsys.CertMdul", "launcher"),
    ("maxon-creative", "net.maxon.python", "cinema-4d"),
    (
        "maxon-creative",
        "net.maxon.cinema4d.installer.uninstall",
        "cinema-4d",
    ),
    ("steinberg-creative", "com.Syncrosoft.LCC", "elicenser"),
    (
        "steinberg-creative",
        "net.steinberg.eLicenser.installer.SteinbergDownloadAssistant.uninstall",
        "download-assistant",
    ),
    (
        "vocaloid",
        "com.apple.ScriptEditor.id.VOCALOID_Authorizer-Uninstaller",
        "authorizer",
    ),
    (
        "vocaloid",
        "com.apple.ScriptEditor.id.VOCALOID-Uninstaller",
        "editor",
    ),
    (
        "vocaloid",
        "com.apple.ScriptEditor.id.VOCALOID4-API-Uninstaller",
        "editor",
    ),
    ("remote-desktop", "barrier", "barrier"),
    (
        "remote-desktop",
        "com.google.chromeremotedesktop.me2me-host-uninstaller",
        "chrome-remote-desktop",
    ),
    ("macos-development", "com.google.antigravity", "antigravity"),
    ("macos-development", "com.microsoft.VSCode", "vscode"),
    (
        "macos-development",
        "com.todesktop.230313mzl4w4u92",
        "cursor",
    ),
    ("macos-development", "com.DanPristupov.Fork", "fork"),
    ("macos-development", "com.googlecode.iterm2", "iterm"),
    (
        "macos-development",
        "com.googlecode.iterm2.iTermBrowserPlugin",
        "iterm",
    ),
    ("macos-development", "dev.kiro.desktop", "kiro"),
    ("macos-development", "ai.opencode.desktop", "opencode"),
    ("macos-development", "com.luckymarmot.Paw", "rapidapi"),
    (
        "macos-development",
        "com.scicotherapylabs.RemoteCodetrolServer",
        "remote-codetrol",
    ),
    ("macos-development", "com.tinyapp.TablePlus", "tableplus"),
    (
        "macos-development",
        "com.microsoft.visual-studio",
        "visual-studio",
    ),
    ("macos-development", "com.apple.dt.Xcode", "xcode"),
    ("macos-development", "dev.zcode.app", "zcode"),
    (
        "macos-ai-assistants",
        "com.google.Chrome.app.hbblfifohofgngfbjbiimbbcimepbdcb",
        "chatbot-ui",
    ),
    ("macos-ai-assistants", "com.openai.codex", "chatgpt-codex"),
    (
        "macos-ai-assistants",
        "ai.elementlabs.lmstudio",
        "lm-studio",
    ),
    ("macos-ai-assistants", "com.goodsnooze.MenuGPT", "macgpt"),
    ("macos-ai-assistants", "ai.perplexity.mac", "perplexity"),
    (
        "macos-ai-assistants",
        "com.superduper.superwhisper",
        "superwhisper",
    ),
    ("macos-avatar-3d", "jp.plusplus.virtualyoutuber", "3tene"),
    (
        "macos-avatar-3d",
        "org.blenderfoundation.blender",
        "blender",
    ),
    (
        "macos-avatar-3d",
        "com.lookingglassfactory.bridge",
        "looking-glass-bridge",
    ),
    ("macos-avatar-3d", "com.github.nanoem", "nanoem"),
    ("macos-avatar-3d", "unity.Quixel.Mixer", "quixel-mixer"),
    ("macos-avatar-3d", "com.soarhap.vrmposing", "vrm-posing"),
    ("macos-avatar-3d", "net.pixiv.vroid.macosx", "vroid-studio"),
    (
        "macos-avatar-3d",
        "Webcam Motion Capture",
        "webcam-motion-capture",
    ),
    (
        "macos-avatar-3d",
        "info.kwcl.WebcamMotionReceiver",
        "webcam-motion-receiver",
    ),
    ("macos-design", "com.scenegroup.cavalry", "cavalry"),
    ("macos-design", "com.figma.Desktop", "figma"),
    ("macos-design", "dev.pencil.desktop", "pencil"),
    ("macos-image", "com.todesktop.241012ess7yxs0e", "comfyui"),
    ("macos-image", "com.apple.PhotoBooth", "photo-booth"),
    (
        "macos-image",
        "com.apple.GenerativePlaygroundApp",
        "image-playground",
    ),
    (
        "macos-image",
        "media.ics.AnimationImageConverter",
        "animation-image-converter",
    ),
    ("macos-image", "com.apple.Photos", "photos"),
    ("macos-video-live", "com.apple.iMovieApp", "imovie"),
    ("macos-video-live", "de.monospc.Lightkey", "lightkey"),
    ("macos-video-live", "com.obsproject.obs-studio", "obs"),
    ("macos-video-live", "com.wails.obsctl", "obsctl"),
    (
        "macos-music-production",
        "com.supermegaultragroovy.capo3.mac",
        "capo",
    ),
    (
        "macos-music-production",
        "com.apple.garageband10",
        "garageband",
    ),
    (
        "macos-music-production",
        "com.snoize.MIDIMonitor",
        "midi-monitor",
    ),
    ("macos-music-production", "net.hexler.lex", "touchosc"),
    (
        "macos-voice-synthesis",
        "jp.co.crypton.CryptonProductManager",
        "crypton-product-manager",
    ),
    (
        "macos-voice-synthesis",
        "jp.co.crypton.PiaproStudio",
        "piapro-studio",
    ),
    ("macos-voice-synthesis", "jp.tuna2134.sbv2-gui", "sbv2-gui"),
    (
        "macos-voice-synthesis",
        "com.dreamtonics.svstudio2.pro",
        "synthesizer-v",
    ),
    (
        "macos-voice-synthesis",
        "com.dreamtonics.svstudio.basic",
        "synthesizer-v",
    ),
    (
        "macos-voice-synthesis",
        "com.dreamtonics.svstudio.pro",
        "synthesizer-v",
    ),
    ("macos-communication", "com.hnc.Discord", "discord"),
    ("macos-communication", "com.apple.FaceTime", "facetime"),
    ("macos-communication", "jp.naver.line.mac", "line"),
    ("macos-communication", "com.atebits.Tweetie2", "x"),
    ("macos-communication", "us.zoom.xos", "zoom"),
    ("macos-communication", "com.apple.MobileSMS", "messages"),
    ("macos-communication", "com.apple.mail", "mail"),
    ("macos-communication", "com.apple.AddressBook", "contacts"),
    ("macos-productivity", "com.goodnotesapp.x", "goodnotes"),
    ("macos-productivity", "com.hakocat.Koucha", "koucha"),
    ("macos-productivity", "notion.id", "notion"),
    (
        "macos-productivity",
        "notion.id.NotionSafariExtension",
        "notion",
    ),
    ("macos-productivity", "com.apple.iCal", "calendar"),
    ("macos-productivity", "com.gateboxlab.cheerpro", "cheerpro"),
    ("macos-productivity", "com.apple.freeform", "freeform"),
    ("macos-productivity", "com.apple.Notes", "notes"),
    ("macos-productivity", "com.apple.reminders", "reminders"),
    ("macos-productivity", "com.apple.clock", "clock"),
    ("macos-media", "com.apple.TV", "tv"),
    ("macos-media", "org.videolan.vlc", "vlc"),
    (
        "macos-media",
        "com.github.th-ch.youtube-music",
        "youtube-music",
    ),
    ("macos-media", "com.apple.iBooksX", "books"),
    ("macos-media", "com.apple.podcasts", "podcasts"),
    ("macos-media", "com.apple.Music", "music"),
    (
        "macos-files",
        "org.filezilla-project.filezilla",
        "filezilla",
    ),
    ("macos-files", "com.google.drivefs", "google-drive"),
    ("macos-files", "io.mountainduck", "mountain-duck"),
    ("macos-files", "com.nextcloud.desktopclient", "nextcloud"),
    (
        "macos-files",
        "org.filezilla-project.remotedrive",
        "remotedrive",
    ),
    (
        "macos-virtual-machines",
        "com.parallels.desktop.console",
        "parallels",
    ),
    (
        "macos-virtual-machines",
        "com.vmware.fusion",
        "vmware-fusion",
    ),
    (
        "macos-customization",
        "com.hegenberg.BetterTouchTool",
        "bettertouchtool",
    ),
    (
        "macos-customization",
        "com.turbo.LauncherTurbo",
        "launcher-turbo",
    ),
    (
        "macos-customization",
        "com.apple.launchpad.launcher",
        "apple-launchpad",
    ),
    (
        "macos-customization",
        "com.kamakiri1192.launchpad",
        "liquid-start",
    ),
    (
        "macos-customization",
        "waikiki.program.Launchpad",
        "launchpad-plus",
    ),
    ("macos-customization", "lo.cafe.NotchNook", "notchnook"),
    ("macos-customization", "com.kyome.ScreenNote", "screen-note"),
    (
        "macos-customization",
        "com.kyome.ScreenPointer",
        "screen-pointer",
    ),
    (
        "macos-customization",
        "com.pilotmoon.scroll-reverser",
        "scroll-reverser",
    ),
    (
        "macos-devices",
        "com.apple.ScreenContinuity",
        "iphone-mirroring",
    ),
    ("macos-devices", "com.apple.Home", "home"),
    ("macos-devices", "com.apple.findmy", "find-my"),
    ("macos-information", "com.apple.news", "news"),
    ("macos-information", "com.apple.Maps", "maps"),
    ("macos-information", "com.apple.weather", "weather"),
    ("macos-information", "com.apple.stocks", "stocks"),
    ("macos-games", "com.hottagames.nte", "nte"),
    ("macos-games", "com.apple.Chess", "chess"),
    ("macos-games", "com.bilibili.trickcal", "trickcal"),
    ("macos-pdf", "com.adobe.Acrobat.Pro", "acrobat"),
    ("macos-pdf", "com.adobe.distiller", "distiller"),
    ("macos-pdf", "com.apple.Preview", "preview"),
    (
        "macos-utilities",
        "com.daisydiskapp.DaisyDiskStandAlone",
        "daisydisk",
    ),
    (
        "macos-utilities",
        "com.apple.exposelauncher",
        "mission-control",
    ),
    ("macos-utilities", "com.kyome.RunCat", "runcat"),
    ("macos-utilities", "com.apple.siri.launcher", "siri"),
    (
        "macos-utilities",
        "com.apple.backup.launcher",
        "time-machine",
    ),
    ("macos-utilities", "com.apple.shortcuts", "shortcuts"),
    ("macos-utilities", "com.apple.Passwords", "passwords"),
    ("macos-utilities", "com.apple.helpviewer", "tips"),
    // Additional application metadata observed on macOS, 2026-09-30.
    (
        "macos-utilities",
        "com.apple.airport.airportutility",
        "airport-utility",
    ),
    (
        "macos-utilities",
        "com.apple.audio.AudioMIDISetup",
        "audio-midi-setup",
    ),
    (
        "macos-utilities",
        "com.apple.BluetoothFileExchange",
        "bluetooth-file-exchange",
    ),
    (
        "macos-utilities",
        "com.apple.bootcampassistant",
        "boot-camp-assistant",
    ),
    (
        "macos-utilities",
        "com.apple.ColorSyncUtility",
        "colorsync-utility",
    ),
    (
        "macos-utilities",
        "com.apple.DigitalColorMeter",
        "digital-color-meter",
    ),
    ("macos-utilities", "com.apple.grapher", "grapher"),
    (
        "macos-utilities",
        "com.apple.MigrateAssistant",
        "migration-assistant",
    ),
    ("macos-utilities", "com.apple.printcenter", "print-center"),
    (
        "macos-utilities",
        "com.apple.ScriptEditor2",
        "script-editor",
    ),
    (
        "macos-utilities",
        "com.apple.SystemProfiler",
        "system-information",
    ),
    ("macos-utilities", "com.apple.Terminal", "terminal"),
    (
        "macos-utilities",
        "com.apple.VoiceOverUtility",
        "voiceover-utility",
    ),
    ("macos-utilities", "com.apple.Automator", "automator"),
    ("macos-utilities", "com.apple.FontBook", "font-book"),
    (
        "macos-utilities",
        "com.apple.Image_Capture",
        "image-capture",
    ),
    ("macos-accessories", "com.apple.TextEdit", "textedit"),
    (
        "macos-accessories",
        "com.apple.screenshot.launcher",
        "screenshot",
    ),
    (
        "macos-accessories",
        "com.apple.QuickTimePlayerX",
        "quicktime-player",
    ),
    ("macos-accessories", "com.apple.Dictionary", "dictionary"),
    ("macos-accessories", "com.apple.VoiceMemos", "voice-memos"),
    ("macos-accessories", "com.apple.Stickies", "stickies"),
    ("microsoft-office", "com.microsoft.Word", "word"),
    ("microsoft-office", "com.microsoft.Excel", "excel"),
    ("microsoft-office", "com.microsoft.Powerpoint", "powerpoint"),
    (
        "adobe-creative-cloud",
        "com.adobe.AfterEffects.application",
        "after-effects",
    ),
    ("adobe-creative-cloud", "com.adobe.bridge", "bridge"),
    ("adobe-creative-cloud", "com.adobe.InDesign", "indesign"),
    (
        "adobe-creative-cloud",
        "com.adobe.acc.AdobeCreativeCloud",
        "creative-cloud",
    ),
    ("maxon-creative", "net.maxon.commandline", "commandline"),
    (
        "maxon-creative",
        "net.maxon.cinema4dclient",
        "team-render-client",
    ),
    (
        "maxon-creative",
        "net.maxon.cinema4dserver",
        "team-render-server",
    ),
    (
        "steinberg-creative",
        "com.steinberg.vst3.GrooveAgentSEStandalone",
        "groove-agent-se",
    ),
    (
        "steinberg-creative",
        "com.steinberg.vst3.HALionSonicStandalone",
        "halion-sonic",
    ),
    (
        "steinberg-creative",
        "com.steinberg.SteinbergActivationManager",
        "activation-manager",
    ),
    (
        "steinberg-creative",
        "com.steinberg.HALionLibraryManager",
        "library-manager",
    ),
    (
        "steinberg-creative",
        "net.steinberg.elicenser.SteinbergDownloadAssistant",
        "download-assistant",
    ),
    ("jetbrains-ides", "com.jetbrains.rustrover", "rustrover"),
    ("jetbrains-ides", "com.jetbrains.air", "air"),
    ("unity-tools", "com.unity3d.unityhub", "hub"),
    ("unity-tools", "com.unity3d.UnityEditor5.x", "editor"),
    ("unity-tools", "com.unity3d.bugreporter", "bug-reporter"),
    (
        "live2d-cubism",
        "com.live2d.cubism.CECubismEditorApp",
        "editor",
    ),
    (
        "live2d-cubism",
        "com.live2d.cubism.doc.modeling.ui.viewerForOriginalWorkflow.OWViewerDropFrame",
        "viewer",
    ),
    ("remote-desktop", "tv.parsec.www", "parsec"),
    (
        "remote-desktop",
        "com.apple.ScreenSharing",
        "screen-sharing",
    ),
    (
        "google-workspace",
        "com.google.drivefs.shortcuts.docs",
        "docs",
    ),
    (
        "google-workspace",
        "com.google.drivefs.shortcuts.sheets",
        "sheets",
    ),
    (
        "google-workspace",
        "com.google.drivefs.shortcuts.slides",
        "slides",
    ),
    ("clip-studio", "jp.co.celsys.CLIPSTUDIO", "launcher"),
    ("clip-studio", "jp.co.celsys.CLIPSTUDIOPAINT", "paint"),
    ("vocaloid", "com.yamaha.VOCALOID6-Editor", "editor"),
    ("vocaloid", "com.yamaha.VOCALOID-Authorizer", "authorizer"),
    ("jetbrains-ides", "com.jetbrains.toolbox", "toolbox"),
    (
        "macos-utilities",
        "com.apple.ActivityMonitor",
        "activity-monitor",
    ),
    ("macos-utilities", "com.apple.Console", "console"),
    ("macos-utilities", "com.apple.DiskUtility", "disk-utility"),
    ("apple-documents", "com.apple.iWork.Pages", "pages"),
    ("apple-documents", "com.apple.iWork.Numbers", "numbers"),
    ("apple-documents", "com.apple.iWork.Keynote", "keynote"),
    ("apple-documents", "com.apple.Pages", "pages"),
    ("apple-documents", "com.apple.Numbers", "numbers"),
    ("apple-documents", "com.apple.Keynote", "keynote"),
    ("libreoffice", "org.libreoffice.script", "libreoffice"),
    ("adobe-creative-cloud", "com.adobe.Photoshop", "photoshop"),
    (
        "adobe-creative-cloud",
        "com.adobe.illustrator",
        "illustrator",
    ),
    (
        "adobe-creative-cloud",
        "com.adobe.AfterEffects",
        "after-effects",
    ),
    ("adobe-creative-cloud", "com.adobe.Audition", "audition"),
    (
        "affinity-legacy",
        "com.seriflabs.affinitydesigner",
        "designer",
    ),
    ("affinity-legacy", "com.seriflabs.affinityphoto", "photo"),
    (
        "affinity-legacy",
        "com.seriflabs.affinitypublisher",
        "publisher",
    ),
    (
        "affinity-legacy",
        "com.seriflabs.affinitydesigner2",
        "designer",
    ),
    ("affinity-legacy", "com.seriflabs.affinityphoto2", "photo"),
    (
        "affinity-legacy",
        "com.seriflabs.affinitypublisher2",
        "publisher",
    ),
    ("coreldraw-suite", "com.corel.CorelDRAW", "draw"),
    (
        "cyberlink-director-suite",
        "com.cyberlink.powerdirectormac",
        "powerdirector",
    ),
    (
        "cyberlink-director-suite",
        "com.cyberlink.photodirectormac",
        "photodirector",
    ),
    ("final-cut-suite", "com.apple.FinalCut", "final-cut"),
    ("final-cut-suite", "com.apple.FinalCutApp", "final-cut"),
    ("final-cut-suite", "com.apple.motionapp", "motion"),
    ("final-cut-suite", "com.apple.motionappApp", "motion"),
    ("final-cut-suite", "com.apple.Compressor", "compressor"),
    ("final-cut-suite", "com.apple.CompressorApp", "compressor"),
    ("logic-suite", "com.apple.logic10", "logic"),
    ("logic-suite", "com.apple.mainstage3", "mainstage"),
    (
        "blackmagic-creative",
        "com.blackmagic-design.DaVinciResolve",
        "resolve",
    ),
    (
        "blackmagic-creative",
        "com.blackmagic-design.DaVinciResolveLite",
        "resolve",
    ),
    (
        "blackmagic-creative",
        "com.blackmagic-design.fusion",
        "fusion",
    ),
    ("maxon-creative", "net.maxon.cinema4d", "cinema-4d"),
    ("maxon-creative", "com.pixologic.zbrush", "zbrush"),
    ("steinberg-creative", "com.steinberg.cubase", "cubase"),
    (
        "native-instruments-standalone",
        "com.native-instruments.Kontakt Application",
        "kontakt",
    ),
    (
        "native-instruments-standalone",
        "com.native-instruments.Reaktor 6",
        "reaktor",
    ),
    ("jetbrains-ides", "com.jetbrains.intellij", "idea"),
    ("jetbrains-ides", "com.jetbrains.intellij.ce", "idea"),
    ("jetbrains-ides", "com.jetbrains.pycharm", "pycharm"),
    ("jetbrains-ides", "com.jetbrains.pycharm.ce", "pycharm"),
    ("jetbrains-ides", "com.jetbrains.WebStorm", "webstorm"),
    ("jetbrains-ides", "com.jetbrains.PhpStorm", "phpstorm"),
    ("jetbrains-ides", "com.jetbrains.CLion", "clion"),
    ("jetbrains-ides", "com.jetbrains.goland", "goland"),
    ("jetbrains-ides", "com.jetbrains.rider", "rider"),
    ("jetbrains-ides", "com.jetbrains.rubymine", "rubymine"),
    ("jetbrains-ides", "com.jetbrains.datagrip", "datagrip"),
];

const MAC_VERSIONED_BUNDLES: &[(&str, &str, &str)] = &[
    ("steinberg-creative", "com.steinberg.cubasesoft", "cubase"),
    (
        "adobe-creative-cloud",
        "com.adobe.PremierePro.",
        "premiere-pro",
    ),
    ("adobe-creative-cloud", "com.adobe.Audition.", "audition"),
    (
        "adobe-creative-cloud",
        "com.adobe.ame.application.",
        "media-encoder",
    ),
    ("steinberg-creative", "com.steinberg.cubase", "cubase"),
    ("steinberg-creative", "com.steinberg.dorico", "dorico"),
    ("steinberg-creative", "net.steinberg.WaveLab-", "wavelab"),
    (
        "native-instruments-standalone",
        "com.native-instruments.Kontakt ",
        "kontakt",
    ),
    ("autodesk-creative", "com.autodesk.AutoCAD", "autocad"),
    ("autodesk-creative", "com.autodesk.Maya.", "maya"),
];
