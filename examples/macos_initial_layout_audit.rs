//! Read-only audit of installed apps through the production macOS scanner.
//! Run: cargo run --example macos_initial_layout_audit > target/macos-layout-audit.json
//! No launcher database, saved layout or application bundle is modified.

#[cfg(target_os = "macos")]
pub use launchpad_windows::{domain, icons};

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/platform/macos/apps.rs"]
mod macos_apps;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/workers/steam_scan.rs"]
mod steam_scan;

#[cfg(target_os = "macos")]
fn main() {
    use domain::initial_layout::{
        self,
        catalog::{Catalog, Classification, Platform},
        Decision,
    };
    use domain::settings::Settings;

    // Mirror workers/app_scan.rs, including the Steam provider. Auditing only
    // .app bundles understates the production snapshot.
    let macos_apps::ApplicationBundleAudit {
        applications: mut snapshot,
        complete: applications_complete,
        bundles,
    } = macos_apps::audit_application_bundles();
    let (steam, steam_errors) = steam_scan::scan_steam_apps_checked();
    for entry in steam {
        snapshot.insert(entry.app_id.clone(), entry);
    }
    let catalog = Catalog::bundled(None);
    let proposal = initial_layout::generate(
        &snapshot,
        &Settings::default(),
        Platform::Macos,
        true,
        "read-only-audit".into(),
        &catalog,
    );
    let folders: Vec<_> = proposal
        .state
        .folders
        .values()
        .map(|folder| {
            let apps: Vec<_> = folder
                .children
                .iter()
                .map(|id| {
                    let e = &snapshot[id];
                    serde_json::json!({"name": e.name, "bundle": e.identifier})
                })
                .collect();
            serde_json::json!({"name": folder.name, "apps": apps})
        })
        .collect();
    let top_level: Vec<_> = proposal
        .state
        .top_level_app_ids()
        .map(|id| {
            let e = &snapshot[id];
            serde_json::json!({"name": e.name, "bundle": e.identifier})
        })
        .collect();
    let entries: Vec<_> = snapshot
        .values()
        .map(|entry| {
            let (preset, product) = match catalog.classify(entry, Platform::Macos) {
                Some(Classification::Product { preset, product }) => (Some(preset), Some(product)),
                _ => (None, None),
            };
            let (decision, reason) = match &proposal.decisions[&entry.app_id] {
                Decision::Grouped(_) => ("grouped", "verified_product_or_purpose"),
                Decision::KeepTopLevel if preset.is_some() => {
                    ("top_level", "fewer_than_two_products")
                }
                Decision::KeepTopLevel => ("top_level", "explicit_daily_entry"),
                Decision::Unresolved => ("unresolved", "unknown_identifier_or_ambiguous_rules"),
            };
            serde_json::json!({
                "id": entry.app_id, "name": entry.name, "bundle": entry.identifier,
                "provider": if entry.app_id.is_steam() {
                    if entry.link_path.ends_with(".app") { "steam_shortcut" } else { "steam_manifest" }
                } else { "macos" },
                "preset": preset, "product": product, "decision": decision, "reason": reason,
            })
        })
        .collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let bundles: Vec<_> = bundles
        .into_iter()
        .map(|(path, outcome)| {
            let location = home
                .as_ref()
                .and_then(|home| path.strip_prefix(home).ok())
                .map(|relative| format!("~/{}", relative.display()))
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            serde_json::json!({"location": location, "result": outcome})
        })
        .collect();
    let report = serde_json::json!({
        "catalog_version": initial_layout::catalog::VERSION,
        "scan_complete": applications_complete && steam_errors.is_empty(),
        "applications_complete": applications_complete,
        "steam_complete": steam_errors.is_empty(),
        "detected_apps": snapshot.len(),
        "top_level_apps": top_level,
        "folders": folders,
        "entries": entries,
        "bundles": bundles,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos-initial-layout-audit: this tool is macOS-only.");
    std::process::exit(1);
}
