//! The public tag database — the manifest the tags-sharing site
//! (`jugeeya.github.io/tags`) publishes at [`TAGS_MANIFEST_URL`], pairing a
//! submitted in-game tag with its owner's start.gg handle. This is a port of
//! what the Cloudflare broker's `getTagNameMap` (broker/worker.js) already
//! does, so a station or hub running with no Cloudflare in the loop at all
//! still gets the same tag -> start.gg translations for free.
//!
//! Interpretation of the manifest mirrors the broker exactly:
//! `{"tags": [{"name": "...", "startgg": {"tag": "..."}}, ...]}` (extra
//! fields ignored, a missing `tags` array treated as empty, an entry with no
//! `startgg.tag` skipped). The broker also only ever overwrites its KV cache
//! when the resulting map is non-empty (`if (Object.keys(map).length) ...`);
//! [`TagDb`] does the same thing for its on-disk cache — a temporarily-empty
//! or unreachable/malformed manifest just means "keep serving what we had",
//! never "blank out everyone's tags".
//!
//! Tournament venues routinely have bad or no internet, so:
//! * [`TagDb::load`] never touches the network — it only reads whatever was
//!   cached from the last successful fetch, so a station with no
//!   connectivity at all still starts up instantly with last night's data.
//! * refreshing happens on its own background thread
//!   ([`TagDb::spawn_refresh`]), at most every [`REFRESH_INTERVAL_S`]; never
//!   on the hot path (the engine poll loop just reads whatever [`TagDb::map`]
//!   currently holds).
//! * a failed or unrecognisable fetch is silently swallowed — logged, but the
//!   existing map/cache is left exactly as it was.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::matching;

pub const TAGS_MANIFEST_URL: &str = "https://jugeeya.github.io/tags/data/index.json";
const CACHE_FILE: &str = "tagdb-cache.json";

/// Don't hit the network more than this often — a tournament's tag roster
/// doesn't meaningfully change minute to minute.
const REFRESH_INTERVAL_S: u64 = 6 * 60 * 60;
/// How often the background thread wakes up to check whether a refresh is
/// due. Much shorter than [`REFRESH_INTERVAL_S`] so a fresh install picks up
/// its first fetch promptly instead of waiting for the full interval.
const CHECK_INTERVAL_S: u64 = 5 * 60;

pub type LogFn = Box<dyn Fn(&str) + Send + Sync>;

/// The save-tag -> start.gg-handle map from the public tag database, with an
/// on-disk cache and a background refresher.
///
/// Cheap to read from on the hot path (`map()` just clones the in-memory
/// map); the network only ever runs on its own thread, on its own schedule.
pub struct TagDb {
    map: Mutex<HashMap<String, String>>,
    cache_path: PathBuf,
    /// When the in-memory map was last populated by a successful fetch (or,
    /// at startup, the cache file's own mtime — see `load`).
    fetched_at: Mutex<Option<SystemTime>>,
}

impl TagDb {
    /// Load whatever is cached on disk next to the config — no network call,
    /// so this is safe to run inline at startup and must never block it.
    /// A missing/unreadable/corrupt cache just means starting with an empty
    /// map; the next background refresh fills it in once the network is
    /// available.
    pub fn load(config_dir: &Path) -> Arc<TagDb> {
        let cache_path = config_dir.join(CACHE_FILE);
        let map = fs::read_to_string(&cache_path)
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, String>>(&t).ok())
            .unwrap_or_default();
        // The cache file's own mtime is a reasonable stand-in for "when this
        // was fetched" across a restart, so relaunching the app doesn't
        // immediately trigger a redundant re-fetch of a still-fresh cache.
        let fetched_at = fs::metadata(&cache_path).and_then(|m| m.modified()).ok();
        Arc::new(TagDb {
            map: Mutex::new(map),
            cache_path,
            fetched_at: Mutex::new(fetched_at),
        })
    }

    /// The current save-tag -> start.gg-handle map (keys normalized the same
    /// way [`matching::build_tag_map`] normalizes its keys).
    pub fn map(&self) -> HashMap<String, String> {
        self.map.lock().unwrap().clone()
    }

    /// Start the background refresher. Returns immediately; the fetch (if
    /// any) happens entirely on the spawned thread, never on the caller's.
    pub fn spawn_refresh(self: &Arc<Self>, log: LogFn) {
        let this = self.clone();
        thread::spawn(move || loop {
            this.maybe_refresh(&log);
            thread::sleep(Duration::from_secs(CHECK_INTERVAL_S));
        });
    }

    fn due_for_refresh(&self) -> bool {
        match *self.fetched_at.lock().unwrap() {
            Some(t) => {
                t.elapsed().unwrap_or(Duration::MAX) > Duration::from_secs(REFRESH_INTERVAL_S)
            }
            None => true,
        }
    }

    fn maybe_refresh(&self, log: &LogFn) {
        if !self.due_for_refresh() {
            return;
        }
        let result = fetch_manifest(TAGS_MANIFEST_URL);
        self.apply_fetch_result(result, log);
    }

    /// Applies the outcome of a manifest fetch. Split out from
    /// `maybe_refresh` so the decision logic (when to persist, when to leave
    /// the cache alone) is testable without ever making a real HTTP call.
    fn apply_fetch_result(&self, result: Result<HashMap<String, String>, String>, log: &LogFn) {
        match result {
            // Same guard the broker uses before writing its KV cache: only a
            // non-empty result is worth persisting. A manifest that's
            // temporarily empty, missing its `tags` array, or otherwise
            // unrecognizable all land here as an empty map — and all of them
            // should mean "keep serving the last good cache", not "everyone's
            // tags are gone".
            Ok(map) if !map.is_empty() => {
                *self.map.lock().unwrap() = map.clone();
                *self.fetched_at.lock().unwrap() = Some(SystemTime::now());
                if let Err(e) = write_cache(&self.cache_path, &map) {
                    log(&format!("could not write tag database cache: {e}"));
                }
            }
            Ok(_) => {}
            Err(e) => log(&format!("tag database refresh failed: {e}")),
        }
    }
}

/// Atomic write (tmp + rename), mirroring the pattern `hub.rs` uses for
/// `hub-state.json` / `learned-tags.json`.
fn write_cache(path: &Path, map: &HashMap<String, String>) -> std::io::Result<()> {
    let body = serde_json::to_string_pretty(map).unwrap_or_else(|_| "{}".to_string());
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

/// GET the manifest and parse it into a tag map. `Err` is a transport
/// failure (offline, non-2xx, timeout, unparsable JSON body at all) — the
/// caller logs it and moves on. A body that parses as JSON but doesn't look
/// like the manifest shape is NOT an error here (see [`parse_manifest`]); it
/// comes back as `Ok` with an empty map, same as the broker's `|| []`.
fn fetch_manifest(url: &str) -> Result<HashMap<String, String>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("tag database unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("tag database HTTP {}", resp.status().as_u16()));
    }
    let body: Value = resp
        .json()
        .map_err(|_| "tag database returned invalid JSON".to_string())?;
    Ok(parse_manifest(&body))
}

/// Same interpretation as the broker's `getTagNameMap`:
/// ```js
/// for (const t of manifest.tags || []) {
///   const sgg = t && t.startgg && t.startgg.tag;
///   if (t && t.name && sgg) map[normChar(t.name)] = sgg;
/// }
/// ```
/// Unknown/extra fields on each entry are ignored; an entry missing a
/// start.gg handle (or a name) is skipped rather than falling back to a
/// guess. A missing/non-array `tags` field is treated as an empty list, not
/// an error — the caller (`apply_fetch_result`) is what decides an empty
/// result shouldn't overwrite a good cache.
fn parse_manifest(body: &Value) -> HashMap<String, String> {
    let empty = Vec::new();
    let tags = body.get("tags").and_then(Value::as_array).unwrap_or(&empty);
    let mut map = HashMap::new();
    for t in tags {
        let name = t.get("name").and_then(Value::as_str);
        let sgg = t
            .get("startgg")
            .and_then(|s| s.get("tag"))
            .and_then(Value::as_str);
        if let (Some(name), Some(sgg)) = (name, sgg) {
            if name.is_empty() || sgg.is_empty() {
                continue;
            }
            let key = matching::norm(&Value::String(name.to_string()));
            if !key.is_empty() {
                map.insert(key, sgg.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tagdbtest_{}_{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn noop_log() -> LogFn {
        Box::new(|_m: &str| {})
    }

    // ---- manifest parsing (pure, no network) -----------------------------------

    #[test]
    fn parse_manifest_extracts_name_to_startgg_tag() {
        let body = json!({
            "tags": [
                {"name": "JUGZ!", "startgg": {"tag": "jugeeya"}, "extra": "ignored"},
                {"name": "KIM", "startgg": {"tag": "Kimchi"}},
            ]
        });
        let map = parse_manifest(&body);
        assert_eq!(map.get("jugz"), Some(&"jugeeya".to_string()));
        assert_eq!(map.get("kim"), Some(&"Kimchi".to_string()));
    }

    #[test]
    fn parse_manifest_skips_entries_missing_a_startgg_handle() {
        let body = json!({
            "tags": [
                {"name": "NOBODY"},
                {"name": "EMPTYTAG", "startgg": {"tag": ""}},
                {"startgg": {"tag": "noname"}},
            ]
        });
        assert!(
            parse_manifest(&body).is_empty(),
            "no entry has both a name and a real start.gg handle"
        );
    }

    #[test]
    fn parse_manifest_treats_a_missing_tags_array_as_empty() {
        assert!(parse_manifest(&json!({})).is_empty());
        assert!(parse_manifest(&json!({"tags": "not an array"})).is_empty());
        assert!(parse_manifest(&json!("just a string")).is_empty());
    }

    // ---- cache-hit-without-network ---------------------------------------------

    #[test]
    fn load_serves_the_cached_map_with_no_network_call() {
        let dir = tmpdir();
        let cached: HashMap<String, String> = [("jugz".to_string(), "jugeeya".to_string())]
            .into_iter()
            .collect();
        fs::write(
            dir.join(CACHE_FILE),
            serde_json::to_string(&cached).unwrap(),
        )
        .unwrap();

        // `load` only ever reads this file; there is no code path here that
        // could reach the network, so a correct result proves the cache
        // (not a fetch) answered.
        let db = TagDb::load(&dir);
        assert_eq!(db.map(), cached);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_with_no_cache_file_starts_with_an_empty_map() {
        let dir = tmpdir();
        let db = TagDb::load(&dir); // nothing written to `dir` at all
        assert!(db.map().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- malformed-payload-leaves-cache ------------------------------------------

    #[test]
    fn empty_fetch_result_leaves_the_existing_cache_and_map_intact() {
        let dir = tmpdir();
        let original: HashMap<String, String> = [("jugz".to_string(), "jugeeya".to_string())]
            .into_iter()
            .collect();
        let cache_path = dir.join(CACHE_FILE);
        fs::write(&cache_path, serde_json::to_string(&original).unwrap()).unwrap();
        let db = TagDb::load(&dir);

        // Simulates what a malformed/empty manifest (or a transport error)
        // produces, without making a real HTTP call.
        db.apply_fetch_result(Ok(HashMap::new()), &noop_log());
        assert_eq!(
            db.map(),
            original,
            "in-memory map untouched by an empty result"
        );
        let on_disk: HashMap<String, String> =
            serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        assert_eq!(on_disk, original, "cache file on disk untouched");

        db.apply_fetch_result(Err("offline".to_string()), &noop_log());
        assert_eq!(
            db.map(),
            original,
            "in-memory map untouched by a transport error"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_good_fetch_result_updates_both_the_map_and_the_cache_file() {
        let dir = tmpdir();
        let db = TagDb::load(&dir); // no pre-existing cache
        let fresh: HashMap<String, String> = [("newtag".to_string(), "brandnew".to_string())]
            .into_iter()
            .collect();

        db.apply_fetch_result(Ok(fresh.clone()), &noop_log());

        assert_eq!(db.map(), fresh);
        let on_disk: HashMap<String, String> =
            serde_json::from_str(&fs::read_to_string(dir.join(CACHE_FILE)).unwrap()).unwrap();
        assert_eq!(on_disk, fresh, "the fresh map was persisted to disk");

        let _ = fs::remove_dir_all(&dir);
    }
}
