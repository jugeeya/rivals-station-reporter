//! Station-side forwarder — port of `station_sender.py`'s `Sender`.
//!
//! Watches the output folder the set machine writes and forwards to a hub (or
//! the Cloudflare broker — same API), stamping this machine's station number:
//!
//! * new  `<dir>/sets/*.json`     -> POST `<broker>/matchlogger/ingest`
//! * changed `<dir>/current.json` -> POST `<broker>/matchlogger/current`
//! * changed `<dir>/live.json`    -> POST `<broker>/matchlogger/live`
//!
//! It DOES hold one secret — the shared key — since the running-score push
//! writes to start.gg automatically, no human involved; it's the same value as
//! the hub/broker's OPERATOR_KEY, not a separate lower-stakes one. It does NOT
//! let this station finalize a set on its own — naming a winner always
//! requires an explicit operator click.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::set_machine::LogFn;

const STATE_VERSION: i64 = 1;

/// Reduce a pasted start.gg event/bracket URL to the broker's event slug.
///
/// The wire wants exactly `tournament/<t>/event/<e>`. People naturally paste
/// the whole URL (with https://www.start.gg/ and a trailing /brackets/… path),
/// so pull the tournament+event pair out of whatever they gave us.
pub fn normalize_slug(slug: &str) -> String {
    let re = regex::Regex::new(r"(?i)tournament/([^/?#]+)/event/([^/?#]+)").unwrap();
    if let Some(m) = re.captures(slug) {
        return format!("tournament/{}/event/{}", &m[1], &m[2]);
    }
    slug.trim().to_string()
}

pub struct Forwarder {
    broker: String,
    slug: String,
    station: i64,
    key: Option<String>,
    out_dir: PathBuf,
    sets_dir: PathBuf,
    current_path: PathBuf,
    live_path: PathBuf,
    state_path: PathBuf,
    dry_run: bool,
    log: LogFn,
    client: reqwest::blocking::Client,
    state: Value,
}

impl Forwarder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        broker: &str,
        slug: &str,
        station: i64,
        out_dir: &Path,
        state_path: Option<&Path>,
        dry_run: bool,
        key: Option<&str>,
        log: LogFn,
    ) -> Self {
        let out_dir = out_dir.to_path_buf();
        let state_path = state_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| out_dir.join(".station-sender-state.json"));
        let mut f = Self {
            broker: broker.trim_end_matches('/').to_string(),
            slug: normalize_slug(slug),
            station,
            key: key.filter(|k| !k.is_empty()).map(|k| k.to_string()),
            sets_dir: out_dir.join("sets"),
            current_path: out_dir.join("current.json"),
            live_path: out_dir.join("live.json"),
            out_dir,
            state_path,
            dry_run,
            log,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
            state: Value::Null,
        };
        f.state = f.load_state();
        f
    }

    pub fn station(&self) -> i64 {
        self.station
    }

    pub fn slug(&self) -> &str {
        &self.slug
    }

    pub fn out_dir(&self) -> &Path {
        &self.out_dir
    }

    // -- persistence --------------------------------------------------------
    fn load_state(&self) -> Value {
        if let Ok(text) = std::fs::read_to_string(&self.state_path) {
            if let Ok(s) = serde_json::from_str::<Value>(&text) {
                if s["version"].as_i64() == Some(STATE_VERSION) {
                    let mut s = s;
                    if !s["sent_sets"].is_array() {
                        s["sent_sets"] = json!([]);
                    }
                    return s;
                }
            }
        }
        json!({ "version": STATE_VERSION, "sent_sets": [], "current_hash": null })
    }

    fn save_state(&self) {
        if self.dry_run {
            return;
        }
        let tmp = self.state_path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(&self.state).unwrap_or_else(|_| "{}".into());
        if std::fs::write(&tmp, body)
            .and_then(|_| std::fs::rename(&tmp, &self.state_path))
            .is_err()
        {
            (self.log)(&format!(
                "could not write state {}",
                self.state_path.display()
            ));
        }
    }

    // -- work helpers ---------------------------------------------------------
    fn payload(&self, extra: Value) -> Value {
        let mut p = json!({ "slug": self.slug, "station": self.station });
        if let Some(k) = &self.key {
            p["key"] = json!(k);
        }
        if let (Some(obj), Some(ex)) = (p.as_object_mut(), extra.as_object()) {
            for (k, v) in ex {
                obj.insert(k.clone(), v.clone());
            }
        }
        p
    }

    // -- network --------------------------------------------------------------
    fn post(&self, endpoint: &str, payload: &Value) -> bool {
        let url = format!("{}{}", self.broker, endpoint);
        if self.dry_run {
            let body = serde_json::to_string_pretty(payload).unwrap_or_default();
            (self.log)(&format!(
                "DRY-RUN POST {url}\n{}",
                &body[..body.len().min(800)]
            ));
            return true;
        }
        // An explicit User-Agent is required: Cloudflare's bot rules 403 the
        // default library signature (error 1010) before the request ever
        // reaches the Worker.
        let res = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "rivals-station-sender/1.0")
            .json(payload)
            .send();
        match res {
            Ok(resp) if resp.status().is_success() => true,
            Ok(resp) => {
                (self.log)(&format!("POST {endpoint} -> HTTP {}", resp.status()));
                false
            }
            Err(e) => {
                (self.log)(&format!("POST {endpoint} failed: {e} (will retry)"));
                false
            }
        }
    }

    // -- work -------------------------------------------------------------
    fn read_json(path: &Path) -> Option<Value> {
        // A None here is normal: the producer may be mid-write when we poll,
        // so we simply retry on the next pass.
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn hash_bytes(bytes: &[u8]) -> String {
        // Content-change detection only (Python used sha1); any stable hash works.
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        format!("{:x}", h.finish())
    }

    fn process_sets(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.sets_dir) else {
            return;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();
        let sent: Vec<String> = self.state["sent_sets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for name in names {
            if sent.contains(&name) {
                continue;
            }
            let Some(body) = Self::read_json(&self.sets_dir.join(&name)) else {
                continue;
            };
            if self.post("/matchlogger/ingest", &self.payload(json!({ "set": body }))) {
                (self.log)(&format!("ingested {name}"));
                self.state["sent_sets"]
                    .as_array_mut()
                    .expect("sent_sets array")
                    .push(json!(name));
                self.save_state();
            }
        }
    }

    fn process_current(&mut self) {
        let Ok(raw) = std::fs::read(&self.current_path) else {
            return;
        };
        let digest = Self::hash_bytes(&raw);
        if self.state["current_hash"].as_str() == Some(digest.as_str()) {
            return; // unchanged since last heartbeat
        }
        let Ok(body) = serde_json::from_slice::<Value>(&raw) else {
            return;
        };
        if self.post(
            "/matchlogger/current",
            &self.payload(json!({ "current": body })),
        ) {
            (self.log)(&format!(
                "heartbeat: {}",
                body["state"].as_str().unwrap_or("?")
            ));
            self.state["current_hash"] = json!(digest);
            self.save_state();
        }
    }

    fn process_live(&mut self) {
        // Running per-game snapshot -> live (non-finalizing) start.gg score.
        let Ok(raw) = std::fs::read(&self.live_path) else {
            return;
        };
        let digest = Self::hash_bytes(&raw);
        if self.state["live_hash"].as_str() == Some(digest.as_str()) {
            return;
        }
        let Ok(body) = serde_json::from_slice::<Value>(&raw) else {
            return;
        };
        // The producer writes {"complete": true} when the set ends — nothing to push.
        if body["complete"].as_bool() != Some(true) {
            if !self.post("/matchlogger/live", &self.payload(json!({ "set": body }))) {
                return; // retry next pass; leave the hash so a later change re-sends
            }
            (self.log)(&format!(
                "live: {} game(s)",
                body["matchCount"]
                    .as_i64()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into())
            ));
        }
        self.state["live_hash"] = json!(digest);
        self.save_state();
    }

    pub fn tick(&mut self) {
        // Heartbeat first: it's time-sensitive (drives start.gg pre-binding).
        self.process_current();
        self.process_live();
        self.process_sets();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_normalization() {
        assert_eq!(
            normalize_slug("https://www.start.gg/tournament/foo-1/event/bar-2/brackets/12/34"),
            "tournament/foo-1/event/bar-2"
        );
        assert_eq!(
            normalize_slug("tournament/a/event/b"),
            "tournament/a/event/b"
        );
        assert_eq!(normalize_slug("  plain  "), "plain");
    }

    #[test]
    fn dry_run_marks_sets_sent_in_memory_only() {
        let dir = std::env::temp_dir().join(format!("fwdtest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sets")).unwrap();
        std::fs::write(
            dir.join("sets/set_x.json"),
            r#"{"setId":"x","complete":true}"#,
        )
        .unwrap();
        std::fs::write(dir.join("current.json"), r#"{"state":"idle"}"#).unwrap();

        let logged = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let l2 = logged.clone();
        let mut f = Forwarder::new(
            "http://example.invalid",
            "tournament/t/event/e",
            3,
            &dir,
            None,
            true, // dry run: post() never touches the network
            Some("k"),
            Box::new(move |m| l2.lock().unwrap().push(m.to_string())),
        );
        f.tick();
        let lines = logged.lock().unwrap().join("\n");
        assert!(lines.contains("DRY-RUN POST http://example.invalid/matchlogger/current"));
        assert!(lines.contains("ingested set_x.json"));
        // dry-run never persists state
        assert!(!dir.join(".station-sender-state.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn payload_carries_slug_station_key() {
        let dir = std::env::temp_dir().join(format!("fwdtest2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = Forwarder::new(
            "http://x/",
            "tournament/t/event/e",
            7,
            &dir,
            None,
            true,
            Some("secret"),
            Box::new(|_| {}),
        );
        let p = f.payload(json!({"set": {"a": 1}}));
        assert_eq!(p["slug"], "tournament/t/event/e");
        assert_eq!(p["station"], 7);
        assert_eq!(p["key"], "secret");
        assert_eq!(p["set"]["a"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
