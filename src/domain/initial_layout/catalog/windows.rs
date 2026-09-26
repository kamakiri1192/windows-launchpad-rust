//! Windows roles verified from local Shell IDs and launch targets. Labels are not identities.
use super::{basename, entry_shell_id, windows_path, SnapshotEntry};

const ACCESSORIES: Option<&str> = Some("windows-accessories");
const MANAGEMENT: Option<&str> = Some("windows-tools");

pub(in super::super) fn identify(
    entry: &SnapshotEntry,
    root: Option<&str>,
) -> Option<(&'static str, Option<&'static str>)> {
    if entry
        .launch_arguments
        .as_deref()
        .is_none_or(|args| args.trim().is_empty())
    {
        if let Some(id) = entry_shell_id(entry) {
            if let Some((_, product, group)) = SHELL_APPS.iter().find(|(known, _, _)| *known == id)
            {
                return Some((*product, *group));
            }
        }
    }
    let root = root?;
    let path = windows_path(&entry.target_path);
    let args = entry.launch_arguments.as_deref()?.trim();
    let file = basename(&path);
    let system_binary = ["system32", "syswow64"]
        .iter()
        .any(|arch| path == format!("{root}\\{arch}\\{file}"));

    // .msc documents have no EXE publisher resource; accept only specific OS paths.
    if system_binary && matches!(args, "" | "/s") {
        if let Some(key) = management_console(file) {
            return Some((key, MANAGEMENT));
        }
    }
    if entry.publisher != "Microsoft Corporation" {
        return None;
    }
    if system_binary && file == "mmc.exe" {
        let arg = args.strip_suffix(" /s").unwrap_or(args);
        let console = if let Some(quoted) = arg.strip_prefix('"') {
            quoted.strip_suffix('"')?
        } else {
            arg
        };
        let console = windows_path(console);
        let name = basename(&console);
        if console == format!("{root}\\system32\\{name}") {
            return management_console(name).map(|key| (key, MANAGEMENT));
        }
    }
    if args.is_empty()
        && ["system32", "syswow64"]
            .iter()
            .any(|arch| path == format!("{root}\\{arch}\\windowspowershell\\v1.0\\{file}"))
    {
        return match file {
            "powershell.exe" => Some(("powershell", ACCESSORIES)),
            "powershell_ise.exe" => Some(("powershell-ise", ACCESSORIES)),
            _ => None,
        };
    }
    if path == format!("{root}\\explorer.exe") && args.is_empty() {
        return Some(("explorer", None));
    }
    if path == format!("{root}\\notepad.exe") && args.is_empty() {
        return Some(("notepad", ACCESSORIES));
    }
    if path == format!("{root}\\regedit.exe") && args.is_empty() {
        return Some(("regedit", MANAGEMENT));
    }
    if !system_binary {
        return None;
    }
    match (file, args) {
        ("calc.exe", "") => Some(("calculator", None)),
        ("notepad.exe", "") => Some(("notepad", ACCESSORIES)),
        ("snippingtool.exe", "") => Some(("snipping", ACCESSORIES)),
        ("mspaint.exe", "") => Some(("paint", ACCESSORIES)),
        ("cmd.exe", "") => Some(("command-prompt", ACCESSORIES)),
        ("charmap.exe", "") => Some(("character-map", ACCESSORIES)),
        ("taskmgr.exe", "" | "/7") => Some(("task-manager", ACCESSORIES)),
        ("control.exe", "") => Some(("control-panel", MANAGEMENT)),
        ("control.exe", "/name Microsoft.AdministrativeTools") => {
            Some(("administrative-tools", MANAGEMENT))
        }
        ("perfmon.exe", "" | "/res") => Some(("performance-monitor", MANAGEMENT)),
        ("dfrgui.exe", "") => Some(("optimize-drives", MANAGEMENT)),
        ("cleanmgr.exe", "") => Some(("disk-cleanup", MANAGEMENT)),
        ("iscsicpl.exe", "") => Some(("iscsi", MANAGEMENT)),
        ("mdsched.exe", "") => Some(("memory-diagnostic", MANAGEMENT)),
        ("odbcad32.exe", "") => Some(("odbc", MANAGEMENT)),
        ("recoverydrive.exe", "") => Some(("recovery-drive", MANAGEMENT)),
        ("regedit.exe", "") => Some(("regedit", MANAGEMENT)),
        ("msconfig.exe", "") => Some(("system-configuration", MANAGEMENT)),
        ("msinfo32.exe", "") => Some(("system-information", MANAGEMENT)),
        ("psr.exe", "") => Some(("steps-recorder", MANAGEMENT)),
        ("rundll32.exe", "x264vfw.dll,Configure" | "x264vfw64.dll,Configure") => {
            Some(("x264-settings", MANAGEMENT))
        }
        ("rundll32.exe", _) => {
            let dll = args.strip_suffix(",GlobalConfigDialog")?;
            let dll = windows_path(dll);
            ["system32", "syswow64"]
                .iter()
                .any(|arch| dll == format!("{root}\\{arch}\\utv_core.dll"))
                .then_some(("utvideo-settings", MANAGEMENT))
        }
        _ => None,
    }
}

fn management_console(file: &str) -> Option<&'static str> {
    Some(match file {
        "compmgmt.msc" => "computer-management",
        "eventvwr.msc" => "event-viewer",
        "comexp.msc" => "component-services",
        "services.msc" => "services",
        "wf.msc" => "firewall",
        "taskschd.msc" => "task-scheduler",
        "perfmon.msc" => "performance-monitor",
        "secpol.msc" => "security-policy",
        "printmanagement.msc" => "print-management",
        _ => return None,
    })
}

// Get-StartApps / AppsFolder identities observed on Windows, 2026-09-25.
const SHELL_APPS: &[(&str, &str, Option<&str>)] = &[
    ("Microsoft.Windows.Explorer", "explorer", None),
    ("Microsoft.WindowsStore_8wekyb3d8bbwe!App", "store", None),
    (
        "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel",
        "settings",
        None,
    ),
    (
        "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
        "calculator",
        None,
    ),
    ("OpenAI.Codex_2p2nqsd0c76g0!App", "chatgpt", None),
    (
        "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0!ChatGPT",
        "chatgpt-classic",
        None,
    ),
    (
        "Microsoft.XboxGamingOverlay_8wekyb3d8bbwe!App",
        "game-bar",
        None,
    ),
    (
        "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App",
        "notepad",
        ACCESSORIES,
    ),
    (
        "Microsoft.ScreenSketch_8wekyb3d8bbwe!App",
        "snipping",
        ACCESSORIES,
    ),
    ("Microsoft.Paint_8wekyb3d8bbwe!App", "paint", ACCESSORIES),
    (
        "Microsoft.WindowsAlarms_8wekyb3d8bbwe!App",
        "clock",
        ACCESSORIES,
    ),
    (
        "Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe!App",
        "sound-recorder",
        ACCESSORIES,
    ),
    (
        "Microsoft.MicrosoftStickyNotes_8wekyb3d8bbwe!App",
        "sticky-notes",
        ACCESSORIES,
    ),
    (
        "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App",
        "terminal",
        ACCESSORIES,
    ),
    (
        "Microsoft.Windows.Shell.RunDialog",
        "run-dialog",
        ACCESSORIES,
    ),
    (
        "Microsoft.Windows.ControlPanel",
        "control-panel",
        MANAGEMENT,
    ),
    (
        "Microsoft.SecHealthUI_8wekyb3d8bbwe!SecHealthUI",
        "security",
        MANAGEMENT,
    ),
    (
        "MicrosoftWindows.Client.CBS_cw5n1h2txyewy!WindowsBackup",
        "backup",
        MANAGEMENT,
    ),
    (
        "Microsoft.GetHelp_8wekyb3d8bbwe!App",
        "get-help",
        MANAGEMENT,
    ),
    (
        "Microsoft.WindowsFeedbackHub_8wekyb3d8bbwe!App",
        "feedback",
        MANAGEMENT,
    ),
    (
        "NVIDIACorp.NVIDIAControlPanel_56jybvy8sckqj!NVIDIACorp.NVIDIAControlPanel",
        "nvidia-settings",
        MANAGEMENT,
    ),
];
