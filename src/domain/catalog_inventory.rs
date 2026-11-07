//! Explicit administrator export: identities only, never local paths or launch arguments.
use super::{
    app_diff::SnapshotEntry,
    initial_layout::catalog::{Catalog, Classification, Platform, Rule},
};
use serde::Serialize;
#[derive(Serialize)]
pub struct InventoryApp {
    pub source_id: String,
    pub name: String,
    pub platform: Platform,
    pub rule: Option<Rule>,
    pub suggested_folder_id: Option<String>,
    pub category: Option<String>,
    pub icon_png: Option<String>,
    pub keep_top_level: bool,
}
pub fn from_entry(e: &SnapshotEntry, platform: Platform, catalog: &Catalog) -> InventoryApp {
    let classification = catalog.classify(e, platform);
    let product = match &classification {
        Some(Classification::Product { product, .. }) => product.clone(),
        _ => e.identifier.clone(),
    };
    let rule = if e.app_id.is_steam() {
        Some(Rule::SteamApp {
            id: e.app_id.to_string(),
        })
    } else if platform == Platform::Macos && !e.identifier.is_empty() {
        Some(Rule::MacBundle {
            bundle: e.identifier.clone(),
            product: product.clone(),
        })
    } else if platform == Platform::Windows {
        if let Some(id) = e
            .link_path
            .strip_prefix(r"shell:AppsFolder\")
            .filter(|s| s.contains('!'))
        {
            Some(Rule::WindowsPackage {
                aumid: id.into(),
                product: id.into(),
            })
        } else {
            let file = e
                .target_path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or("")
                .to_lowercase();
            (e.launch_arguments.as_deref() == Some("")
                && file.ends_with(".exe")
                && !e.publisher.is_empty()
                && !e.identifier.is_empty()
                && !matches!(
                    file.as_str(),
                    "cmd.exe"
                        | "powershell.exe"
                        | "pwsh.exe"
                        | "rundll32.exe"
                        | "explorer.exe"
                        | "wscript.exe"
                        | "cscript.exe"
                        | "msiexec.exe"
                ))
            .then_some(Rule::WindowsMetadata {
                executable: file,
                publisher: e.publisher.clone(),
                product_name: e.identifier.clone(),
                product,
            })
        }
    } else {
        None
    };
    let identity = serde_json::to_string(&rule).unwrap_or_default();
    InventoryApp {
        source_id: identity,
        name: e.name.clone(),
        platform,
        rule,
        suggested_folder_id: match classification {
            Some(Classification::Product { preset, .. }) => Some(preset.into()),
            _ => None,
        },
        category: None,
        icon_png: None,
        keep_top_level: matches!(classification, Some(Classification::KeepTopLevel)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_metadata_without_local_paths_or_commands() {
        let mut e = SnapshotEntry {
            app_id: super::super::app_id::AppId::from_normalized("private-path"),
            name: "Editor".into(),
            link_path: "C:/Users/private/Desktop/Editor.lnk".into(),
            target_path: "C:/Users/private/Editor/editor.exe".into(),
            launch_arguments: Some("".into()),
            source_modified_at: 0,
            target_modified_at: 0,
            icon_location: "private.ico".into(),
            icon_index: 0,
            provider_icon: None,
            version: "1".into(),
            publisher: "Vendor".into(),
            identifier: "Editor".into(),
        };
        let item = from_entry(&e, Platform::Windows, &Catalog::bundled(None));
        assert!(matches!(item.rule, Some(Rule::WindowsMetadata { .. })));
        assert!(!serde_json::to_string(&item).unwrap().contains("private"));
        e.launch_arguments = Some("--token secret".into());
        let item = from_entry(&e, Platform::Windows, &Catalog::bundled(None));
        assert!(item.rule.is_none());
        assert!(!serde_json::to_string(&item).unwrap().contains("secret"));
    }
}
