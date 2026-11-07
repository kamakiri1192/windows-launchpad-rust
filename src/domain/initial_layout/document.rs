//! Versioned public catalog contract. Untrusted downloads are validated before use.
use super::catalog::{Catalog, Platform, Preset, Rule};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDocument {
    pub schema_version: u32,
    pub version: u32,
    pub published_at: Option<String>,
    pub folders: Vec<FolderDefinition>,
    pub apps: Vec<AppDefinition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderDefinition {
    pub id: String,
    pub names: BTreeMap<String, String>,
    pub platforms: Vec<Platform>,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppDefinition {
    pub id: String,
    pub name: String,
    pub folder_id: String,
    pub enabled: bool,
    pub rules: Vec<Rule>,
    pub icon: Option<IconReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconReference {
    pub path: String,
    pub source: String,
}
fn text_valid(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn id_valid(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 160
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
impl CatalogDocument {
    pub fn bundled() -> Self {
        Self::parse(include_bytes!("../../../assets/catalog/catalog-v1.json"))
            .expect("valid bundled catalog")
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("catalog exceeds 2 MiB".into());
        }
        let doc: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        doc.validate()?;
        Ok(doc)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.version == 0
            || self.folders.is_empty()
            || self.folders.len() > 200
            || self.apps.len() > 10000
        {
            return Err("unsupported catalog version or size".into());
        }
        let mut folders = BTreeSet::new();
        for f in &self.folders {
            if !id_valid(&f.id)
                || !folders.insert(&f.id)
                || f.platforms.is_empty()
                || f.platforms.len() > 2
                || !["en", "ja"]
                    .iter()
                    .all(|k| f.names.get(*k).is_some_and(|v| text_valid(v, 160)))
                || f.names.len() > 32
                || f.names
                    .iter()
                    .any(|(k, v)| !text_valid(k, 32) || !text_valid(v, 160))
                || !text_valid(&f.evidence, 2048)
            {
                return Err(format!("invalid folder: {}", f.id));
            }
        }
        let mut ids = BTreeSet::new();
        let mut identities = BTreeMap::new();
        for app in &self.apps {
            if !id_valid(&app.id)
                || !ids.insert(&app.id)
                || !folders.contains(&app.folder_id)
                || !text_valid(&app.name, 160)
                || app.rules.is_empty()
                || app.rules.len() > 100
            {
                return Err(format!("invalid application: {}", app.id));
            }
            if let Some(icon) = &app.icon {
                let hash = icon
                    .path
                    .strip_prefix("icons/")
                    .and_then(|s| s.strip_suffix(".png"));
                if !hash.is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                    || !text_valid(&icon.source, 2048)
                {
                    return Err("invalid icon reference".into());
                }
            }
            for rule in &app.rules {
                let mut value = serde_json::to_value(rule).map_err(|e| e.to_string())?;
                let object = value.as_object_mut().expect("tagged rule");
                if object
                    .values()
                    .any(|v| v.as_str().is_some_and(|s| !text_valid(s, 512)))
                {
                    return Err(format!("empty or oversized rule: {}", app.id));
                }
                let kind = object["type"].as_str().expect("rule tag");
                let folder = self
                    .folders
                    .iter()
                    .find(|f| f.id == app.folder_id)
                    .expect("checked folder");
                let required = if kind.starts_with("mac_") {
                    Some(Platform::Macos)
                } else if kind.starts_with("steam") {
                    None
                } else {
                    Some(Platform::Windows)
                };
                if required.is_some_and(|os| !folder.platforms.contains(&os)) {
                    return Err(format!("rule OS does not match folder: {}", app.id));
                }
                let product = object.remove("product");
                // Same identity in different folders/products must never be resolved by ordering.
                let identity = serde_json::to_string(&object).map_err(|e| e.to_string())?;
                if app.enabled {
                    let output = (app.folder_id.clone(), product);
                    if identities
                        .insert(identity, output.clone())
                        .is_some_and(|previous| previous != output)
                    {
                        return Err(format!("conflicting rule: {}", app.id));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn into_catalog(self, windows_directory: Option<String>) -> Catalog {
        let presets = self
            .folders
            .into_iter()
            .map(|f| {
                let rules = self
                    .apps
                    .iter()
                    .filter(|a| a.enabled && a.folder_id == f.id)
                    .flat_map(|a| a.rules.clone())
                    .collect();
                Preset {
                    id: f.id,
                    japanese_name: f.names["ja"].clone(),
                    english_name: f.names["en"].clone(),
                    platforms: f.platforms,
                    evidence: f.evidence,
                    rules,
                }
            })
            .collect();
        Catalog {
            version: self.version,
            presets,
            windows_directory: windows_directory
                .map(|p| p.replace('/', "\\").trim_end_matches('\\').to_lowercase()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_round_trip_and_reject_conflicting_identity() {
        let mut doc = CatalogDocument::bundled();
        CatalogDocument::parse(&serde_json::to_vec(&doc).unwrap()).unwrap();
        let mut duplicate = doc
            .apps
            .iter()
            .find(|a| a.rules.iter().any(|r| matches!(r, Rule::MacBundle { .. })))
            .unwrap()
            .clone();
        duplicate.id = "conflict".into();
        duplicate.folder_id = "steam-library".into();
        doc.apps.push(duplicate);
        assert!(doc.validate().unwrap_err().contains("conflicting"));
    }
    #[test]
    fn rejects_unknown_schema_dangling_folder_and_path() {
        let mut doc = CatalogDocument::bundled();
        doc.schema_version = 2;
        assert!(doc.validate().is_err());
        doc.schema_version = 1;
        doc.apps[0].folder_id = "missing".into();
        assert!(doc.validate().is_err());
        let mut doc = CatalogDocument::bundled();
        doc.apps[0].icon = Some(IconReference {
            path: "../config.php".into(),
            source: "upload".into(),
        });
        assert!(doc.validate().is_err());
        assert!(CatalogDocument::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
}
