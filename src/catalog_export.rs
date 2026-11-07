//! Owner-invoked local file export; deliberately no network/upload behavior.
use crate::{
    domain::{
        catalog_inventory::from_entry,
        initial_layout::catalog::{Catalog, Platform},
    },
    workers::{
        app_scan,
        icon_worker::{IconReason, IconRequest},
    },
};
use base64::Engine;
use std::{io::Cursor, path::Path};
pub fn export(path: &Path) -> Result<usize, String> {
    #[cfg(windows)]
    let _com = crate::icons::extract::ComScope::new();
    let (snapshot, status) = app_scan::scan_start_menu_checked();
    let catalog = Catalog::bundled(std::env::var("SystemRoot").ok());
    let mut apps = Vec::new();
    for e in snapshot.values() {
        let mut item = from_entry(e, Platform::current(), &catalog);
        #[cfg(target_os = "macos")]
        {
            item.category =
                plist::Value::from_file(Path::new(&e.link_path).join("Contents/Info.plist"))
                    .ok()
                    .and_then(|p| {
                        p.as_dictionary()
                            .and_then(|d| d.get("LSApplicationCategoryType"))
                            .and_then(|v| v.as_string())
                            .map(str::to_owned)
                    });
        }
        let req = IconRequest {
            app_id: e.app_id.clone(),
            name: e.name.clone(),
            link_path: e.link_path.clone().into(),
            source_modified_at: e.source_modified_at,
            target_path: e.target_path.clone(),
            target_modified_at: e.target_modified_at,
            icon_location: e.icon_location.clone(),
            icon_index: e.icon_index,
            provider_icon: e.provider_icon.clone(),
            reason: IconReason::Fresh,
        };
        if let Some(raw) = crate::workers::icon_worker::extract_request_icon(&req) {
            if let Some(image) = image::RgbaImage::from_raw(raw.w, raw.h, raw.rgba) {
                let image = image::DynamicImage::ImageRgba8(image).thumbnail(96, 96);
                let mut bytes = Cursor::new(Vec::new());
                if image.write_to(&mut bytes, image::ImageFormat::Png).is_ok() {
                    item.icon_png =
                        Some(base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()));
                }
            }
        }
        apps.push(item);
    }
    let output = serde_json::json!({"schema_version":1,"source":"desktop","complete":status.complete(),"apps":apps});
    // Never silently replace an existing export.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(&output).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(snapshot.len())
}
