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
fn main() {
    use domain::initial_layout::{
        self,
        catalog::{Catalog, Platform},
    };
    use domain::settings::Settings;

    let (snapshot, complete) = macos_apps::scan_applications_checked();
    let proposal = initial_layout::generate(
        &snapshot,
        &Settings::default(),
        Platform::Macos,
        true,
        "read-only-audit".into(),
        &Catalog::bundled(None),
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
    let report = serde_json::json!({
        "catalog_version": initial_layout::catalog::VERSION,
        "scan_complete": complete,
        "detected_apps": snapshot.len(),
        "top_level_apps": top_level,
        "folders": folders,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos-initial-layout-audit: this tool is macOS-only.");
    std::process::exit(1);
}
