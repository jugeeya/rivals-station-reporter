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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::set_machine::LogFn;

const STATE_VERSION: i64 = 1;

/// Classifies why a POST to the hub/broker failed, so the operator sees
/// *why* nothing is arriving instead of just "something went wrong" - a bad
/// shared key and an unreachable hub have completely different fixes (check
/// the Key field vs check the Hub/broker URL and that the hub is running).
#[derive(Debug, Clone, PartialEq)]
enum ForwardFailureKind {
    /// The request never got a response at all (DNS failure, connection
    /// refused, timeout) - the hub/broker isn't there or isn't reachable.
    Unreachable,
    /// 401/403: the shared key doesn't match the hub/broker's.
    BadKey,
    /// Any other non-2xx status.
    Http(u16),
}

impl ForwardFailureKind {
    fn message(&self, endpoint: &str) -> String {
        match self {
            ForwardFailureKind::Unreachable => format!(
                "cannot reach the hub/broker for {endpoint}: check the Hub/broker URL \
                 and that the hub is running"
            ),
            ForwardFailureKind::BadKey => format!(
                "the hub/broker rejected the shared key for {endpoint} (401/403): \
                 check the Key field matches the hub/broker's"
            ),
            ForwardFailureKind::Http(code) => {
                format!("the hub/broker returned HTTP {code} for {endpoint}")
            }
        }
    }
}

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

    // -- connectivity health (see `forward_status`) --------------------
    consecutive_failures: u32,
    last_failure: Option<(ForwardFailureKind, String)>, // (kind, endpoint)

    // -- clock skew against the hub (see `clock_skew_check`) ------------
    last_clock_check: Option<Instant>,
    last_skew_s: Option<i64>,
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
            consecutive_failures: 0,
            last_failure: None,
            last_clock_check: None,
            last_skew_s: None,
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
    fn post(&mut self, endpoint: &str, payload: &Value) -> bool {
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
            Ok(resp) if resp.status().is_success() => {
                self.note_success();
                true
            }
            Ok(resp) => {
                let status = resp.status();
                (self.log)(&format!("POST {endpoint} -> HTTP {status}"));
                let kind = if status.as_u16() == 401 || status.as_u16() == 403 {
                    ForwardFailureKind::BadKey
                } else {
                    ForwardFailureKind::Http(status.as_u16())
                };
                self.note_failure(kind, endpoint);
                false
            }
            Err(e) => {
                (self.log)(&format!("POST {endpoint} failed: {e} (will retry)"));
                self.note_failure(ForwardFailureKind::Unreachable, endpoint);
                false
            }
        }
    }

    /// A POST succeeded: reset the failure streak so `forward_status` stops
    /// reporting a problem the instant the hub answers again, rather than
    /// waiting for some number of consecutive successes.
    fn note_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure = None;
    }

    fn note_failure(&mut self, kind: ForwardFailureKind, endpoint: &str) {
        self.consecutive_failures += 1;
        self.last_failure = Some((kind, endpoint.to_string()));
    }

    /// Don't warn on a single blip - a hub restarting mid-event (an operator
    /// bouncing it between rounds) produces a failure or two that clear up
    /// on their own and shouldn't alarm anyone. Three in a row rules that
    /// out while still catching a real outage within a few poll intervals.
    pub const FORWARD_FAIL_WARN_THRESHOLD: u32 = 3;

    /// `Some(message)` once consecutive failures cross
    /// `FORWARD_FAIL_WARN_THRESHOLD` (stays `Some`, describing the latest
    /// failure kind, for as long as they keep failing); `None` otherwise,
    /// including immediately after any single success. The engine latches
    /// this the same way it latches the Replay Auto Save warning - see
    /// `EngineInner::check_forward_health`.
    pub fn forward_status(&self) -> Option<String> {
        if self.consecutive_failures < Self::FORWARD_FAIL_WARN_THRESHOLD {
            return None;
        }
        self.last_failure
            .as_ref()
            .map(|(kind, endpoint)| kind.message(endpoint))
    }

    // -- clock skew -----------------------------------------------------------

    /// Half of `StatsProducer::REPLAY_WINDOW_S` (45s -> ~22s): a station this
    /// far off already burns half the margin that window has over the
    /// measured real-world detection-to-replay lag (9-16s), so this warns
    /// well before replay matching actually starts failing at the full
    /// window.
    pub const CLOCK_SKEW_WARN_S: i64 = crate::set_machine::StatsProducer::REPLAY_WINDOW_S / 2;

    /// Skew is a static property of a misconfigured clock, not something
    /// that wanders tick to tick, so re-probing every engine poll would just
    /// be needless LAN chatter. This only has to be frequent enough that
    /// fixing the clock (or an NTP resync) clears the warning without a
    /// restart.
    const CLOCK_CHECK_INTERVAL_S: u64 = 60;

    /// Probe the hub/broker's own clock (via `/matchlogger/health`'s
    /// `serverTime`) and return this station's skew against it in seconds
    /// (positive = this station is ahead), accounting for request
    /// round-trip so ordinary network latency isn't mistaken for skew - see
    /// `skew_from_roundtrip`.
    ///
    /// Returns `None` when there's nothing to report either way: dry run (no
    /// real hub to ask), still within the throttle window (returns the last
    /// known reading instead, so a caller polling every tick doesn't see it
    /// flicker to "unknown" between real probes), the request failed
    /// (unreachable - indistinguishable here from "no hub configured", which
    /// is also nothing to warn about), or the response has no `serverTime`
    /// field (an older hub, or the Cloudflare broker, neither of which
    /// implements this).
    pub fn clock_skew_check(&mut self) -> Option<i64> {
        if self.dry_run {
            return None;
        }
        if let Some(last) = self.last_clock_check {
            if last.elapsed() < Duration::from_secs(Self::CLOCK_CHECK_INTERVAL_S) {
                return self.last_skew_s;
            }
        }
        self.last_clock_check = Some(Instant::now());
        self.last_skew_s = self.probe_clock_skew();
        self.last_skew_s
    }

    fn probe_clock_skew(&self) -> Option<i64> {
        let sent = SystemTime::now();
        let url = format!("{}/matchlogger/health", self.broker);
        let resp = self.client.get(&url).send().ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let received = SystemTime::now();
        let body: Value = resp.json().ok()?;
        let server_time = body.get("serverTime").and_then(|v| v.as_i64())?;
        Some(Self::skew_from_roundtrip(server_time, sent, received))
    }

    /// Pure so the round-trip math is testable without a network: `sent` and
    /// `received` bracket the request on this station's clock, and
    /// `server_time` is the hub's own clock reading, taken by the hub
    /// somewhere in between. The midpoint of `sent`/`received` is the best
    /// single-request estimate of what this station's clock read at the
    /// instant the hub took its reading, so that midpoint - not either raw
    /// endpoint - is what gets compared against it; otherwise ordinary
    /// network latency would show up as apparent skew.
    fn skew_from_roundtrip(server_time: i64, sent: SystemTime, received: SystemTime) -> i64 {
        let sent_s = sent
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let received_s = received
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let local_mid = (sent_s + received_s) / 2.0;
        (server_time as f64 - local_mid).round() as i64
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
        // Dry run never touches the network, so it can never accumulate a
        // failure streak or have anything to warn about.
        assert_eq!(f.forward_status(), None);
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

    /// A `Forwarder` pointed at a URL that never actually gets touched
    /// (`note_failure`/`note_success` are driven directly below) - just
    /// something valid to construct with. Each call gets its own temp dir
    /// (an atomic counter, not just the shared process id) since several
    /// tests below call this concurrently and a shared dir would race on
    /// `remove_dir_all`/`create_dir_all`.
    fn test_forwarder(dry_run: bool) -> Forwarder {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fwdtest_health_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Forwarder::new(
            "http://example.invalid",
            "tournament/t/event/e",
            1,
            &dir,
            None,
            dry_run,
            Some("k"),
            Box::new(|_| {}),
        )
    }

    // -- forward-failure tracking ---------------------------------------------

    #[test]
    fn a_couple_of_failures_do_not_warn_yet() {
        let mut f = test_forwarder(false);
        f.note_failure(ForwardFailureKind::Unreachable, "/matchlogger/current");
        f.note_failure(ForwardFailureKind::Unreachable, "/matchlogger/current");
        assert_eq!(
            f.forward_status(),
            None,
            "a hub restarting mid-event shouldn't trip a warning over one or two blips"
        );
    }

    #[test]
    fn consecutive_failures_past_threshold_warn() {
        let mut f = test_forwarder(false);
        for _ in 0..Forwarder::FORWARD_FAIL_WARN_THRESHOLD {
            f.note_failure(ForwardFailureKind::Unreachable, "/matchlogger/current");
        }
        assert!(f.forward_status().is_some());
    }

    #[test]
    fn a_single_success_clears_the_warning() {
        let mut f = test_forwarder(false);
        for _ in 0..Forwarder::FORWARD_FAIL_WARN_THRESHOLD {
            f.note_failure(ForwardFailureKind::Unreachable, "/matchlogger/current");
        }
        assert!(f.forward_status().is_some());
        f.note_success();
        assert_eq!(
            f.forward_status(),
            None,
            "the very next success should clear it, not require a streak of successes"
        );
    }

    #[test]
    fn bad_key_and_unreachable_produce_different_messages() {
        let mut bad_key = test_forwarder(false);
        for _ in 0..Forwarder::FORWARD_FAIL_WARN_THRESHOLD {
            bad_key.note_failure(ForwardFailureKind::BadKey, "/matchlogger/current");
        }
        let mut unreachable = test_forwarder(false);
        for _ in 0..Forwarder::FORWARD_FAIL_WARN_THRESHOLD {
            unreachable.note_failure(ForwardFailureKind::Unreachable, "/matchlogger/current");
        }

        let key_msg = bad_key.forward_status().unwrap();
        let unreachable_msg = unreachable.forward_status().unwrap();
        assert_ne!(key_msg, unreachable_msg);
        assert!(key_msg.contains("key"), "{key_msg}");
        assert!(unreachable_msg.contains("reach"), "{unreachable_msg}");
    }

    #[test]
    fn other_http_errors_are_reported_with_their_status_code() {
        let mut f = test_forwarder(false);
        for _ in 0..Forwarder::FORWARD_FAIL_WARN_THRESHOLD {
            f.note_failure(ForwardFailureKind::Http(500), "/matchlogger/ingest");
        }
        let msg = f.forward_status().unwrap();
        assert!(msg.contains("500"), "{msg}");
    }

    // -- clock skew -------------------------------------------------------------

    #[test]
    fn skew_from_roundtrip_compensates_for_the_round_trip() {
        let sent = UNIX_EPOCH + Duration::from_secs(1_000);
        let received = UNIX_EPOCH + Duration::from_secs(1_010); // 10s round trip
                                                                // The hub's clock read exactly the local midpoint (1005) at the
                                                                // moment it answered - true skew is zero, even though comparing
                                                                // against `sent` alone would show -5s and against `received` alone
                                                                // would show +5s.
        assert_eq!(Forwarder::skew_from_roundtrip(1_005, sent, received), 0);
    }

    #[test]
    fn skew_below_threshold_is_not_flagged() {
        let sent = UNIX_EPOCH + Duration::from_secs(1_000);
        let received = UNIX_EPOCH + Duration::from_secs(1_000);
        let skew = Forwarder::skew_from_roundtrip(
            1_000 + Forwarder::CLOCK_SKEW_WARN_S - 1,
            sent,
            received,
        );
        assert!(skew.abs() < Forwarder::CLOCK_SKEW_WARN_S);
    }

    #[test]
    fn skew_above_threshold_is_flagged() {
        let sent = UNIX_EPOCH + Duration::from_secs(1_000);
        let received = UNIX_EPOCH + Duration::from_secs(1_000);
        let skew = Forwarder::skew_from_roundtrip(
            1_000 + Forwarder::CLOCK_SKEW_WARN_S + 5,
            sent,
            received,
        );
        assert!(skew.abs() > Forwarder::CLOCK_SKEW_WARN_S);
    }

    #[test]
    fn clock_skew_check_is_none_in_dry_run() {
        let mut f = test_forwarder(true);
        assert_eq!(
            f.clock_skew_check(),
            None,
            "dry run never talks to a real hub, so there's nothing to compare"
        );
    }

    #[test]
    fn clock_skew_check_is_none_when_nothing_answers() {
        // Nothing listens on this port - stands in for "no hub configured"
        // (which, in the running app, means no Forwarder is even built -
        // see engine::build) and for a hub that's simply unreachable right
        // now: either way, no verdict, not a false warning.
        let dir = std::env::temp_dir().join(format!("fwdtest_noskew_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = Forwarder::new(
            "http://127.0.0.1:1",
            "tournament/t/event/e",
            1,
            &dir,
            None,
            false,
            None,
            Box::new(|_| {}),
        );
        assert_eq!(f.clock_skew_check(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
