//! Static catalog delivery. No installed-app data ever leaves the device.
use crate::domain::initial_layout::{
    catalog::Catalog,
    document::{CatalogDocument, MAX_BYTES},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static ACTIVE: OnceLock<RwLock<Option<CatalogDocument>>> = OnceLock::new();
const INTERVAL: u64 = 24 * 60 * 60;
#[derive(Serialize, Deserialize)]
struct Cached {
    source: String,
    etag: Option<String>,
    checked_at: u64,
    document: CatalogDocument,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn active() -> &'static RwLock<Option<CatalogDocument>> {
    ACTIVE.get_or_init(|| RwLock::new(None))
}
pub fn current(windows_directory: Option<String>) -> Catalog {
    active()
        .read()
        .ok()
        .and_then(|v| v.clone())
        .unwrap_or_else(CatalogDocument::bundled)
        .into_catalog(windows_directory)
}
fn read_cache(path: &Path, source: &str) -> Option<Cached> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take((MAX_BYTES + 4096) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let cached: Cached = serde_json::from_slice(&bytes).ok()?;
    (cached.source == source
        && cached.document.version >= super::domain::initial_layout::catalog::VERSION
        && cached.document.validate().is_ok())
    .then_some(cached)
}
fn save(path: &Path, cache: &Cached) -> Result<(), String> {
    let bytes = serde_json::to_vec(cache).map_err(|e| e.to_string())?;
    let pending = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&pending, bytes).map_err(|e| e.to_string())?;
    let result = std::fs::rename(&pending, path).map_err(|e| e.to_string());
    if result.is_err() {
        let _ = std::fs::remove_file(pending);
    }
    result
}
fn accept(previous: &CatalogDocument, bytes: &[u8]) -> Result<CatalogDocument, String> {
    let next = CatalogDocument::parse(bytes)?;
    if next.version < previous.version {
        return Err("catalog version decreased".into());
    }
    if next.version == previous.version
        && serde_json::to_vec(&next).ok() != serde_json::to_vec(previous).ok()
    {
        return Err("catalog changed without a version increment".into());
    }
    Ok(next)
}
fn refresh(path: &Path, source: &str) -> Result<(), String> {
    let mut cached = read_cache(path, source).unwrap_or_else(|| Cached {
        source: source.into(),
        etag: None,
        checked_at: 0,
        document: CatalogDocument::bundled(),
    });
    let clock = now();
    // A future timestamp must not freeze updates indefinitely after a clock adjustment.
    if cached.checked_at <= clock && clock - cached.checked_at < INTERVAL {
        return Ok(());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .into();
    let mut request = agent.get(source).header("Accept", "application/json");
    if let Some(etag) = &cached.etag {
        request = request.header("If-None-Match", etag);
    }
    let result = (|| {
        let mut response = request.call().map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            304 if cached.etag.is_some() => {}
            200 => {
                let bytes = response
                    .body_mut()
                    .with_config()
                    .limit(MAX_BYTES as u64)
                    .read_to_vec()
                    .map_err(|e| e.to_string())?;
                let document = accept(&cached.document, &bytes)?;
                cached.etag = response
                    .headers()
                    .get("etag")
                    .and_then(|h| h.to_str().ok())
                    .filter(|v| v.len() <= 512)
                    .map(str::to_owned);
                cached.document = document;
            }
            status => return Err(format!("unexpected catalog status {status}")),
        }
        Ok(())
    })();
    // Back off on failure too; restarting the app does not hammer the host.
    cached.checked_at = if result.is_ok() {
        clock
    } else {
        clock.saturating_sub(INTERVAL - 3600)
    };
    save(path, &cached)?;
    if result.is_ok() {
        *active().write().map_err(|e| e.to_string())? = Some(cached.document);
    }
    result
}
/// Load the last known good data synchronously, then refresh away from the UI.
/// Endpoint is supplied by the distributor at build time or explicitly by an operator.
pub fn start(directory: PathBuf) {
    let source = std::env::var("LAUNCHPAD_CATALOG_URL")
        .ok()
        .or_else(|| option_env!("LAUNCHPAD_CATALOG_URL").map(str::to_owned));
    let Some(source) = source.filter(|s| !s.is_empty()) else {
        return;
    };
    if !source.starts_with("https://") {
        eprintln!("catalog: endpoint must use HTTPS");
        return;
    }
    let path = directory.join("classification-catalog.json");
    if let Some(cached) = read_cache(&path, &source) {
        if let Ok(mut current) = active().write() {
            *current = Some(cached.document);
        }
    }
    let _ = std::thread::Builder::new()
        .name("catalog-sync".into())
        .spawn(move || loop {
            if let Err(error) = refresh(&path, &source) {
                eprintln!("catalog: keeping offline data: {error}");
            }
            // Small stable process jitter spreads startup cohorts; only one GET per day on success.
            std::thread::sleep(Duration::from_secs(
                3600 + u64::from(std::process::id() % 300),
            ));
        });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_downgrades_same_version_edits_and_bad_payloads() {
        let original = CatalogDocument::bundled();
        let mut next = original.clone();
        next.version -= 1;
        assert!(accept(&original, &serde_json::to_vec(&next).unwrap()).is_err());
        next.version = original.version;
        next.apps[0].name = "changed".into();
        assert!(accept(&original, &serde_json::to_vec(&next).unwrap()).is_err());
        next.version += 1;
        assert!(accept(&original, &serde_json::to_vec(&next).unwrap()).is_ok());
        assert!(accept(&original, b"<html>server error</html>").is_err());
    }
    #[test]
    fn http_refresh_uses_etag_and_retains_last_good_on_invalid_response() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let source = format!("http://{}/catalog.json", listener.local_addr().unwrap());
        let path =
            std::env::temp_dir().join(format!("launchpad-http-test-{}.json", std::process::id()));
        let mut next = CatalogDocument::bundled();
        next.version += 1;
        let body = serde_json::to_string(&next).unwrap();
        let server = std::thread::spawn(move || {
            for (index, response) in [
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"version-8\"\r\nConnection: close\r\n\r\n{}", body.len(), body),
                "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".into(),
                "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops".into(),
            ].iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = [0; 8192];
                let length = stream.read(&mut request).unwrap();
                let headers = String::from_utf8_lossy(&request[..length]).to_lowercase();
                if index > 0 { assert!(headers.contains("if-none-match: \"version-8\"")); }
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        refresh(&path, &source).unwrap();
        assert_eq!(
            read_cache(&path, &source).unwrap().document.version,
            next.version
        );
        for attempt in 0..2 {
            let mut cached = read_cache(&path, &source).unwrap();
            cached.checked_at = 0;
            save(&path, &cached).unwrap();
            let result = refresh(&path, &source);
            assert_eq!(result.is_ok(), attempt == 0);
            assert_eq!(
                read_cache(&path, &source).unwrap().document.version,
                next.version
            );
        }
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        *active().write().unwrap() = None;
    }

    #[test]
    fn cache_is_atomic_source_scoped_and_validated() {
        let path = std::env::temp_dir().join(format!(
            "launchpad-catalog-test-{}.json",
            std::process::id()
        ));
        let cache = Cached {
            source: "https://example.test/catalog.json".into(),
            etag: Some("etag".into()),
            checked_at: now(),
            document: CatalogDocument::bundled(),
        };
        save(&path, &cache).unwrap();
        assert!(read_cache(&path, &cache.source).is_some());
        assert!(read_cache(&path, "https://other.test").is_none());
        std::fs::write(&path, b"truncated").unwrap();
        assert!(read_cache(&path, &cache.source).is_none());
        std::fs::remove_file(path).unwrap();
    }
}
