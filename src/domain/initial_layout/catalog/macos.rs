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
}

const MAC_BUNDLES: &[(&str, &str, &str)] = &[
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
