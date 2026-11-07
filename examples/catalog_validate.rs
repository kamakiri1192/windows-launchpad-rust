//! Validate a published admin JSON using the exact desktop contract.
use launchpad_windows::domain::initial_layout::document::CatalogDocument;
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("catalog_validate <catalog.json>");
    let doc = CatalogDocument::parse(&std::fs::read(path).expect("read catalog"))
        .expect("valid desktop catalog");
    println!(
        "Catalog v{}: {} apps, {} folders",
        doc.version,
        doc.apps.len(),
        doc.folders.len()
    );
}
