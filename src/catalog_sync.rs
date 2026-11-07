//! Static catalog delivery. No installed-app data ever leaves the device.
use crate::domain::initial_layout::{
    catalog::Catalog,
    document::{CatalogDocument, MAX_BYTES},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    io::Read,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static ACTIVE: OnceLock<RwLock<Option<CatalogDocument>>> = OnceLock::new();
const INTERVAL: u64 = 24 * 60 * 60;
const MAX_DELAY: u64 = INTERVAL + INTERVAL / 4;
const STARTUP_MIN: u64 = 30;
const STARTUP_MAX: u64 = 30 * 60;
const OVERDUE_GRACE: u64 = 5 * 60;
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Schedule {
    recorded_at: u64,
    due_at: u64,
    failures: u32,
}
impl Schedule {
    fn valid_at(self, clock: u64) -> bool {
        self.recorded_at <= clock
            && self.due_at >= self.recorded_at
            && self.due_at - self.recorded_at <= MAX_DELAY
    }
}
#[derive(Serialize, Deserialize)]
struct Cached {
    source: String,
    etag: Option<String>,
    checked_at: u64,
    #[serde(default)]
    schedule: Option<Schedule>,
    document: CatalogDocument,
}
fn entropy() -> u64 {
    // Per-call random hash keys, not a user/device identifier; never transmitted.
    RandomState::new().hash_one(())
}
fn retry_delay(failures: u32, random: u64) -> u64 {
    let base = (3600_u64 << failures.saturating_sub(1).min(5)).min(INTERVAL);
    base + random % (base / 4 + 1)
}
impl Cached {
    fn prepare_schedule(&mut self, clock: u64, random: u64, startup: bool) -> bool {
        let valid = self.schedule.filter(|s| s.valid_at(clock));
        if valid.is_some_and(|s| {
            s.due_at > clock || (!startup && clock.saturating_sub(s.due_at) <= OVERDUE_GRACE)
        }) {
            return false;
        }
        // Preserve the remaining 24-hour interval when migrating the old cache format.
        let remaining =
            if self.schedule.is_none() && self.checked_at > 0 && self.checked_at <= clock {
                INTERVAL.saturating_sub(clock - self.checked_at)
            } else {
                0
            };
        let delay = STARTUP_MIN + random % (STARTUP_MAX - STARTUP_MIN + 1);
        self.schedule = Some(Schedule {
            recorded_at: clock,
            due_at: clock.saturating_add(remaining + delay),
            failures: valid.map_or(0, |s| s.failures),
        });
        true
    }
    fn record_attempt(&mut self, clock: u64, random: u64, succeeded: bool) {
        let failures = if succeeded {
            0
        } else {
            self.schedule.map_or(0, |s| s.failures).saturating_add(1)
        };
        let delay = if succeeded {
            INTERVAL + random % (INTERVAL / 4 + 1)
        } else {
            retry_delay(failures, random)
        };
        self.checked_at = clock;
        self.schedule = Some(Schedule {
            recorded_at: clock,
            due_at: clock.saturating_add(delay),
            failures,
        });
    }
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
fn refresh_at(
    path: &Path,
    source: &str,
    clock: u64,
    random: u64,
    startup: bool,
) -> Result<(), String> {
    let mut cached = read_cache(path, source).unwrap_or_else(|| Cached {
        source: source.into(),
        etag: None,
        checked_at: 0,
        schedule: None,
        document: CatalogDocument::bundled(),
    });
    // Persist before contacting the server: repeated restarts keep the same deadline.
    // Cold starts and resume after sleep with an overdue cache also spread out.
    if cached.prepare_schedule(clock, random, startup) {
        return save(path, &cached);
    }
    if cached.schedule.is_some_and(|s| clock < s.due_at) {
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
    cached.record_attempt(clock, random, result.is_ok());
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
        .spawn(move || {
            let mut startup = true;
            loop {
                let result = refresh_at(&path, &source, now(), entropy(), startup);
                if let Err(error) = &result {
                    eprintln!("catalog: keeping offline data: {error}");
                }
                startup = false;
                let clock = now();
                let delay = read_cache(&path, &source)
                    .and_then(|c| c.schedule)
                    .map(|s| {
                        if s.valid_at(clock) {
                            s.due_at.saturating_sub(clock).clamp(1, 3600)
                        } else {
                            1 // Re-schedule promptly after the system clock moves backwards.
                        }
                    })
                    .unwrap_or_else(|| retry_delay(1, entropy()));
                // A full/read-only disk must not turn a failed cache write into one GET per second.
                let minimum = if result.is_err() {
                    retry_delay(1, entropy())
                } else {
                    1
                };
                std::thread::sleep(Duration::from_secs(delay.max(minimum)));
            }
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
        let clock = 1_800_000_000;
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
        refresh_at(&path, &source, clock, 123, true).unwrap();
        let initial = read_cache(&path, &source).unwrap().schedule.unwrap();
        // A restart before the deadline neither contacts the server nor moves the deadline.
        refresh_at(&path, &source, clock + 1, 789, true).unwrap();
        assert_eq!(
            read_cache(&path, &source).unwrap().schedule.unwrap().due_at,
            initial.due_at
        );
        refresh_at(&path, &source, initial.due_at, 123, false).unwrap();
        assert_eq!(
            read_cache(&path, &source).unwrap().document.version,
            next.version
        );
        for attempt in 0..2 {
            let due = read_cache(&path, &source).unwrap().schedule.unwrap().due_at;
            // No request one second before the persisted deadline.
            refresh_at(&path, &source, due - 1, 456, false).unwrap();
            let result = refresh_at(&path, &source, due, 456, false);
            assert_eq!(result.is_ok(), attempt == 0);
            assert_eq!(
                read_cache(&path, &source).unwrap().document.version,
                next.version
            );
            let scheduled = read_cache(&path, &source).unwrap().schedule.unwrap();
            assert_eq!(scheduled.failures, if attempt == 0 { 0 } else { 1 });
            refresh_at(&path, &source, due + 1, 789, true).unwrap();
            assert_eq!(
                read_cache(&path, &source).unwrap().schedule.unwrap().due_at,
                scheduled.due_at
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
            schedule: None,
            document: CatalogDocument::bundled(),
        };
        save(&path, &cache).unwrap();
        assert!(read_cache(&path, &cache.source).is_some());
        assert!(read_cache(&path, "https://other.test").is_none());
        std::fs::write(&path, b"truncated").unwrap();
        assert!(read_cache(&path, &cache.source).is_none());
        std::fs::remove_file(path).unwrap();
    }

    fn offline_cache() -> Cached {
        Cached {
            source: "https://example.test/catalog.json".into(),
            etag: None,
            checked_at: 0,
            schedule: None,
            document: CatalogDocument::bundled(),
        }
    }
    #[test]
    fn startup_spreads_cohorts_and_restarts_preserve_future_deadlines() {
        let clock = 1_800_000_000;
        let mut cache = offline_cache();
        let mut bins = [0; 30];
        for random in 0..100_000 {
            cache.schedule = None;
            assert!(cache.prepare_schedule(clock, random, true));
            let due = cache.schedule.unwrap().due_at;
            assert!((clock + STARTUP_MIN..=clock + STARTUP_MAX).contains(&due));
            bins[((due - clock - STARTUP_MIN) / 60) as usize] += 1;
            assert!(!cache.prepare_schedule(clock + 1, random + 500, true));
            assert_eq!(cache.schedule.unwrap().due_at, due);
        }
        assert!(bins.into_iter().all(|count| count > 1000 && count < 4000));
        // A cohort starting after days offline is staggered again.
        assert!(cache.prepare_schedule(clock + INTERVAL * 3, 90, true));
        assert_eq!(cache.schedule.unwrap().due_at, clock + INTERVAL * 3 + 120);
        // Running processes resuming together after sleep must also avoid a request burst.
        let due = cache.schedule.unwrap().due_at;
        assert!(!cache.prepare_schedule(due + OVERDUE_GRACE, 90, false));
        assert!(cache.prepare_schedule(due + OVERDUE_GRACE + 1, 90, false));
        assert_eq!(
            cache.schedule.unwrap().due_at,
            due + OVERDUE_GRACE + 1 + 120
        );
    }
    #[test]
    fn failed_requests_back_off_exponentially_and_success_resets_backoff() {
        let mut cache = offline_cache();
        let mut clock = 1_800_000_000;
        for expected in [3600, 7200, 14400, 28800, 57600, 86400, 86400] {
            cache.record_attempt(clock, 0, false);
            assert_eq!(cache.schedule.unwrap().due_at - clock, expected);
            clock = cache.schedule.unwrap().due_at;
        }
        assert_eq!(cache.schedule.unwrap().failures, 7);
        cache.record_attempt(clock, INTERVAL / 4, true);
        assert_eq!(cache.schedule.unwrap().failures, 0);
        assert_eq!(cache.schedule.unwrap().due_at - clock, MAX_DELAY);
        for failures in 1..100 {
            let minimum = retry_delay(failures, 0);
            let spread = retry_delay(failures, minimum / 4);
            assert_eq!(spread, minimum + minimum / 4);
            assert!(spread <= MAX_DELAY);
        }
    }
    #[test]
    fn migrates_old_cache_and_recovers_after_clock_rollback() {
        let clock = 1_800_000_000;
        let mut json = serde_json::to_value(offline_cache()).unwrap();
        json.as_object_mut().unwrap().remove("schedule");
        json["checked_at"] = (clock - 3600).into();
        let mut cache: Cached = serde_json::from_value(json).unwrap();
        assert!(cache.prepare_schedule(clock, 0, true));
        assert_eq!(
            cache.schedule.unwrap().due_at,
            clock + INTERVAL - 3600 + STARTUP_MIN
        );
        assert!(cache.prepare_schedule(clock - 3600, 0, false));
        assert_eq!(cache.schedule.unwrap().due_at, clock - 3600 + STARTUP_MIN);
    }
}
