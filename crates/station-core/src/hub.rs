//! LAN hub — the operator's local replacement for the Cloudflare broker.
//!
//! At an event every machine is on the same network, so there is no reason to
//! send per-game traffic to the cloud: the operator runs this, the stations
//! POST to it, and it is the only thing that talks to start.gg. Cloudflare is
//! then not in the loop at all (no KV reads, writes or list ops).
//!
//! It deliberately speaks the SAME `/matchlogger/*` HTTP API as
//! broker/worker.js, so a station switches over by pointing its broker URL at
//! the operator's LAN address — no station code changes:
//!
//! ```text
//! POST /matchlogger/current   {slug, station, key, current}
//! POST /matchlogger/live      {slug, station, key, set}    -> live start.gg score
//! POST /matchlogger/ingest    {slug, station, key, set}
//! GET  /matchlogger/event?slug=...
//! GET  /matchlogger/version?slug=...
//! POST /matchlogger/report    {slug, station, setId, winnerEntrantId, passcode}
//! POST /matchlogger/swap      {slug, station, setId, passcode}
//! POST /matchlogger/delete    {slug, station, setId, passcode}
//! ```
//!
//! Reporting a winner (which advances the bracket) is never automatic — only
//! the operator's explicit action calls it, exactly as with the broker.
//! Per-game live scores DO go out automatically, without setting a winner.
//!
//! (The Python original was stdlib-only so it froze into the station .exe; the
//! Rust port keeps the same shape over `tiny_http`, a thread-per-request server
//! mirroring `ThreadingHTTPServer`.)

use std::collections::HashMap;
use std::fs;
use std::net::UdpSocket;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Map, Value};

use crate::matching;
use crate::now_sec;
use crate::startgg::{Startgg, StartggApi, StartggError, STATION_CACHE_S};
use crate::stats::{is_reportable, mode_label};

pub const DEFAULT_PORT: u16 = 8787;

/// The hub's log callback (Python's `log=` keyword).
pub type LogFn = Box<dyn Fn(&str) + Send + Sync>;
/// Fired with a fresh [`Hub::snapshot`] after every state change.
pub type OnChangeFn = Box<dyn Fn(&Value) + Send + Sync>;

/// This machine's LAN address — what stations should point at.
pub fn lan_ip() -> String {
    let sock = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(_) => return "127.0.0.1".to_string(),
    };
    // no packets sent; just picks the iface
    match sock
        .connect(("10.255.255.255", 1))
        .and_then(|_| sock.local_addr())
    {
        Ok(addr) => addr.ip().to_string(),
        Err(_) => "127.0.0.1".to_string(),
    }
}

// -- Python-shaped helpers ----------------------------------------------------

/// Python truthiness (`if x:`); `None` (an absent key) is falsy.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// Python `str()` over a JSON value: strings pass through unquoted.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python `int()`; `None` where the original raised (missing, unparsable, …).
fn py_int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Python `rec.get(key, True)` truthiness — a missing key defaults to true.
fn get_default_true(rec: &Value, key: &str) -> bool {
    match rec.get(key) {
        None => true,
        Some(v) => truthy(Some(v)),
    }
}

/// `st.get('mode')` as the `Option<&str>`-shaped mode the stats helpers take.
fn mode_of(st: &Value) -> Option<String> {
    match st.get("mode") {
        None | Some(Value::Null) => None,
        Some(v) => Some(py_str(v)),
    }
}

/// The start.gg character map comes over the trait as a JSON object
/// ({norm(name): id}); the matching helpers take a typed map.
fn char_map_of(v: &Value) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if let Some(id) = py_int(Some(val)) {
                out.insert(k.clone(), id);
            }
        }
    }
    out
}

/// Python `Hub._sid`: `'%s:%s' % (station, set_id)`.
fn sid(station: i64, set_id: &Value) -> String {
    format!("{}:{}", station, py_str(set_id))
}

/// `rec.get('ingestedAt') or 0` — the sort key for set listings.
fn ingested(r: &Value) -> i64 {
    r.get("ingestedAt").and_then(|v| v.as_i64()).unwrap_or(0)
}

// -- Hub ------------------------------------------------------------------------

/// Everything the Python `Hub.__init__` kept as mutable attributes, behind one
/// lock (Python used an `RLock`; every `&self` method locks this).
struct HubState {
    tag_map: HashMap<String, String>,
    learned: Map<String, Value>,
    version: i64,
    /// {slug: {station: {...}}}
    stations: Map<String, Value>,
    /// {slug: {"station:setId": record}}
    sets: Map<String, Value>,
}

/// Everything the operator UI renders, newest set first. (A free function so
/// `_touch` can build it while already holding the lock — Python's RLock made
/// the reentrant `self.snapshot()` call safe there.)
fn snapshot_of(s: &HubState) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for bucket in s.sets.values() {
        if let Some(b) = bucket.as_object() {
            for rec in b.values() {
                out.push(rec.clone());
            }
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(ingested(b)));
    json!({"version": s.version, "sets": out, "stations": Value::Object(s.stations.clone())})
}

/// Python `Hub._set_bucket`: `self.sets.setdefault(slug, {})`.
fn set_bucket<'a>(s: &'a mut HubState, slug: &str) -> &'a mut Map<String, Value> {
    s.sets
        .entry(slug.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    s.sets
        .get_mut(slug)
        .and_then(|v| v.as_object_mut())
        .expect("sets bucket is a JSON object")
}

fn load_state(log: &dyn Fn(&str), state_path: Option<&str>, s: &mut HubState) {
    let Some(path) = state_path else { return };
    if !Path::new(path).exists() {
        return;
    }
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log(&format!("could not read hub state: {e}"));
            return;
        }
    };
    let data: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            log(&format!("could not read hub state: {e}"));
            return;
        }
    };
    s.stations = data
        .get("stations")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    s.sets = data
        .get("sets")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let vv = match data.get("version") {
        Some(v) if truthy(Some(v)) => v.clone(),
        _ => json!(0),
    };
    match py_int(Some(&vv)) {
        Some(n) => s.version = n,
        None => {
            // Python: int() raised ValueError -> the same 'could not read' log
            // (stations/sets had already been assigned, exactly as here).
            log(&format!("could not read hub state: invalid version {vv}"));
            return;
        }
    }
    let count: usize = s
        .sets
        .values()
        .map(|b| b.as_object().map(|o| o.len()).unwrap_or(0))
        .sum();
    log(&format!("hub state restored ({count} set(s))"));
}

/// Event state + the start.gg side effects. Transport-agnostic: the HTTP
/// handler and the operator UI both call these methods.
pub struct Hub {
    pub key: Option<String>,
    /// Corrections the operator has made (save tag -> start.gg tag), kept
    /// apart from the hand-written players.json so that file is never
    /// rewritten. Merged over it, so a manual entry can be corrected once.
    pub learned_path: Option<String>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
    on_change: Option<OnChangeFn>,
    pub startgg: Box<dyn StartggApi>,
    pub state_path: Option<String>,
    state: Mutex<HubState>,
}

impl Hub {
    #[allow(clippy::too_many_arguments)] // mirrors the Python __init__ keywords
    pub fn new(
        key: Option<&str>,
        token: Option<String>,
        tag_map: Option<HashMap<String, String>>,
        tagdb_map: Option<HashMap<String, String>>,
        state_path: Option<String>,
        log: Option<LogFn>,
        on_change: Option<OnChangeFn>,
        learned_path: Option<String>,
    ) -> Hub {
        // Python: self.key = (key or '').strip() or None
        let key = key
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(String::from);
        let log: Arc<dyn Fn(&str) + Send + Sync> = match log {
            Some(b) => Arc::from(b),
            None => Arc::new(|_m: &str| {}),
        };
        // Precedence, lowest to highest: the public tag database fills in
        // everyone nobody has corrected yet, the hand-written players.json
        // overrides it for anyone the operator typed in by hand, and (below)
        // a learned correction overrides both.
        let mut tag_map = {
            let mut base = tagdb_map.unwrap_or_default();
            base.extend(tag_map.unwrap_or_default());
            base
        };
        let mut learned: Map<String, Value> = Map::new();
        if let Some(lp) = learned_path.as_deref() {
            if Path::new(lp).exists() {
                let parsed = fs::read_to_string(lp)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok());
                if let Some(Value::Object(o)) = parsed {
                    learned = o;
                    tag_map.extend(matching::build_tag_map(Some(&Value::Object(
                        learned.clone(),
                    ))));
                }
            }
        }
        let sg_log = log.clone();
        let startgg: Box<dyn StartggApi> = Box::new(Startgg::with_log(
            token,
            Some(Box::new(move |m: &str| (sg_log)(m))),
        ));
        let mut state = HubState {
            tag_map,
            learned,
            version: 0,
            stations: Map::new(),
            sets: Map::new(),
        };
        {
            let l = |m: &str| (log)(m);
            load_state(&l, state_path.as_deref(), &mut state);
        }
        Hub {
            key,
            learned_path,
            log,
            on_change,
            startgg,
            state_path,
            state: Mutex::new(state),
        }
    }

    /// Emit to the hub's log callback (Python callers used `hub.log(...)`).
    pub fn log(&self, msg: &str) {
        (self.log)(msg);
    }

    pub fn version(&self) -> i64 {
        self.state.lock().unwrap().version
    }

    /// The current save-tag -> start.gg-tag map (tag database + players.json
    /// + learned, in that precedence order).
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.state.lock().unwrap().tag_map.clone()
    }

    // -- persistence --------------------------------------------------------
    fn save(&self, s: &HubState) {
        let Some(path) = &self.state_path else { return };
        let data = json!({
            "version": s.version,
            "stations": Value::Object(s.stations.clone()),
            "sets": Value::Object(s.sets.clone()),
        });
        let body = serde_json::to_string_pretty(&data).unwrap_or_else(|_| "{}".to_string());
        let tmp = format!("{path}.tmp");
        if let Err(e) = fs::write(&tmp, body).and_then(|_| fs::rename(&tmp, path)) {
            (self.log)(&format!("could not write hub state: {e}"));
        }
    }

    fn touch(&self, s: &mut HubState) {
        s.version += 1;
        self.save(s);
        if let Some(cb) = &self.on_change {
            let snap = snapshot_of(s);
            // Python: try/except pass — an operator-UI error never stops the hub.
            let _ = catch_unwind(AssertUnwindSafe(|| cb(&snap)));
        }
    }

    // -- helpers ------------------------------------------------------------
    /// Stations/console must present the shared key when one is set. On a
    /// LAN this is mostly about catching misconfiguration, but it also stops a
    /// stray machine polluting the event.
    pub fn check_key(&self, supplied: Option<&Value>) -> bool {
        match &self.key {
            None => true,
            Some(k) => {
                // Python: str(supplied or '') == self.key
                let s = match supplied {
                    Some(v) if truthy(Some(v)) => py_str(v),
                    _ => String::new(),
                };
                &s == k
            }
        }
    }

    /// Look up which start.gg set is at this station (entrants, round).
    fn bind_station_set(&self, slug: &str, station: i64) -> Value {
        if !self.startgg.enabled() {
            return Value::Null;
        }
        match self.startgg.station_set(slug, station, STATION_CACHE_S) {
            Ok(v) => v,
            Err(e) => {
                (self.log)(&format!("station lookup failed: {e}"));
                Value::Null
            }
        }
    }

    // -- station-facing -----------------------------------------------------
    pub fn handle_current(
        &self,
        slug: &str,
        station: i64,
        current: Option<&Value>,
    ) -> Result<Value, (Value, u16)> {
        let current = match current {
            Some(v) if v.is_object() => v.clone(),
            _ => json!({}),
        };
        let mut s = self.state.lock().unwrap();
        let mut rec = json!({"station": station, "current": current, "updatedAt": now_sec()});
        // A set just started here -> pre-bind the two entrants now, so the
        // eventual live/ingest can name a winner with far less ambiguity.
        if rec["current"]["state"] == *"set_start" {
            let sg = self.bind_station_set(slug, station);
            if truthy(Some(&sg)) {
                rec["startgg"] = sg;
            }
        } else {
            let prev = s
                .stations
                .get(slug)
                .and_then(|m| m.get(station.to_string()))
                .cloned();
            if let Some(prev) = prev {
                if truthy(prev.get("startgg")) {
                    rec["startgg"] = prev["startgg"].clone();
                }
            }
        }
        s.stations
            .entry(slug.to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("stations bucket is a JSON object")
            .insert(station.to_string(), rec.clone());
        self.touch(&mut s);
        Ok(json!({"ok": true, "startgg": rec.get("startgg")}))
    }

    /// Build/refresh the stored record for a set coming off a station.
    fn record_for(
        &self,
        s: &HubState,
        slug: &str,
        station: i64,
        st: &Value,
        status: &str,
    ) -> (String, Value) {
        let mut sg = Value::Null;
        if let Some(prev_station) = s
            .stations
            .get(slug)
            .and_then(|m| m.get(station.to_string()))
        {
            if truthy(prev_station.get("startgg")) {
                sg = prev_station["startgg"].clone();
            }
        }
        if !truthy(Some(&sg)) {
            sg = self.bind_station_set(slug, station);
        }
        let mut summary = matching::summarize_set(st);
        summary["mode"] = st.get("mode").cloned().unwrap_or(Value::Null);
        let key = sid(station, st.get("setId").unwrap_or(&Value::Null));
        let prev = s
            .sets
            .get(slug)
            .and_then(|b| b.get(&key))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let wm = matching::match_winner(&summary, sg.get("entrants"), Some(&s.tag_map));
        let cand = wm
            .candidate_winner_entrant_id
            .clone()
            .unwrap_or(Value::Null);

        // Two independent reasons a set must stay off the bracket.
        let mode = mode_of(st);
        let mut reason: Option<String> = None;
        if !is_reportable(mode.as_deref()) {
            let lbl = mode_label(mode.as_deref());
            reason = Some(format!(
                "{} game",
                if lbl.is_empty() {
                    "non-local".to_string()
                } else {
                    lbl
                }
            ));
        } else if truthy(Some(&sg)) && !matching::set_started(Some(&sg)) {
            // Called to this station but the TO hasn't pressed Start Match, so
            // anything played here is still a warmup.
            reason = Some("match not started on start.gg".to_string());
        } else if !truthy(Some(&sg)) {
            reason = Some("no start.gg set at this station".to_string());
        }
        let mut rec = json!({
            "id": st.get("setId"),
            "station": station,
            "ingestedAt": now_sec(),
            "set": summary,
            "matchedStartggSetId": sg.get("setId"),
            "fullRoundText": sg.get("fullRoundText"),
            "entrants": sg.get("entrants"),
            "candidateWinnerEntrantId": cand,
            "confidence": wm.confidence,
            "status": if prev["status"] == *"reported" { "reported" } else { status },
            "swap": prev.get("swap").cloned().unwrap_or(json!(false)),
            "mode": st.get("mode"),
            "startggState": sg.get("state"),
            "reportable": reason.is_none(),
            "notReportableReason": reason.clone(),
        });
        // Not a tournament game, or the match isn't underway yet: keep the
        // record (the operator still wants to see it) but don't let it borrow
        // the station's bracket set, or the console would offer to report it.
        if reason.is_some() {
            rec["matchedStartggSetId"] = Value::Null;
            rec["candidateWinnerEntrantId"] = Value::Null;
            rec["confidence"] = json!("none");
            let lbl = mode_label(mode.as_deref());
            rec["status"] = json!(if !lbl.is_empty() {
                lbl
            } else if truthy(Some(&sg)) {
                "waiting for start".to_string()
            } else {
                "recorded".to_string()
            });
        }
        // Preserve anything the operator already decided.
        for k in [
            "reportedAt",
            "reportedWinnerEntrantId",
            "reportedGames",
            "reportedBy",
        ] {
            if let Some(v) = prev.get(k) {
                rec[k] = v.clone();
            }
        }
        if truthy(prev.get("swap")) && truthy(Some(&cand)) {
            let ents = rec.get("entrants").cloned().unwrap_or(Value::Null);
            if let Some(arr) = ents.as_array().filter(|a| a.len() == 2) {
                let cand_s = py_str(&cand);
                if let Some(other) = arr
                    .iter()
                    .find(|e| py_str(e.get("id").unwrap_or(&Value::Null)) != cand_s)
                {
                    rec["candidateWinnerEntrantId"] =
                        other.get("id").cloned().unwrap_or(Value::Null);
                }
            }
        }
        (key, rec)
    }

    /// A running set: store it and push the games-so-far to start.gg
    /// WITHOUT a winner, so the bracket never advances.
    pub fn handle_live(&self, slug: &str, station: i64, st: &Value) -> Result<Value, (Value, u16)> {
        if !st.is_object() {
            return Err((json!({"error": "Missing set."}), 400));
        }
        let (rec, tag_map) = {
            let mut s = self.state.lock().unwrap();
            let (key, mut rec) = self.record_for(&s, slug, station, st, "live");
            // Don't clobber the mode label _record_for set for online/ranked.
            if rec["status"] != *"reported" && get_default_true(&rec, "reportable") {
                rec["status"] = json!("live");
            }
            set_bucket(&mut s, slug).insert(key, rec.clone());
            self.touch(&mut s);
            let tm = s.tag_map.clone();
            (rec, tm)
        };

        let (mut live, mut games) = (false, 0usize);
        let mut reason: Option<String> = None;
        if !get_default_true(&rec, "reportable") {
            let why = match rec.get("notReportableReason") {
                Some(v) if truthy(Some(v)) => py_str(v),
                _ => "not reportable".to_string(),
            };
            reason = Some(format!("{why} — logged, not reported"));
        } else if !self.startgg.enabled() {
            reason = Some("no start.gg token".to_string());
        } else if !truthy(rec.get("matchedStartggSetId")) {
            reason = Some("no matched start.gg set".to_string());
        } else {
            match matching::map_slots_to_entrants(&rec, None, Some(&tag_map)) {
                None => reason = Some("could not map players to entrants".to_string()),
                Some(slot_map) => {
                    let cmap = match self.startgg.character_map(slug) {
                        Ok(v) => char_map_of(&v),
                        // The real client never errors here (it returns a stale
                        // or empty map instead); an error would have escaped the
                        // Python method and become the HTTP handler's 500.
                        Err(e) => return Err((json!({"error": format!("Hub error: {}", e)}), 500)),
                    };
                    let games_v = rec["set"]
                        .get("games")
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    let gd: Vec<Value> = matching::game_data_from_games(&games_v, &slot_map, &cmap)
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|g| truthy(g.get("winnerId")))
                        .collect();
                    if gd.is_empty() {
                        reason = Some("no completed games yet".to_string());
                    } else {
                        match self
                            .startgg
                            .update_live(&rec["matchedStartggSetId"], &Value::Array(gd.clone()))
                        {
                            Ok(()) => {
                                live = true;
                                games = gd.len();
                            }
                            Err(e) => reason = Some(format!("start.gg update failed: {e}")),
                        }
                    }
                }
            }
        }
        if let Some(r) = &reason {
            (self.log)(&format!("live (station {station}): {r}"));
        } else {
            (self.log)(&format!(
                "live (station {station}): pushed {games} game(s) to start.gg"
            ));
        }
        Ok(json!({"ok": true, "live": live, "games": games, "reason": reason}))
    }

    /// A finished set. Stored and matched; never written to the bracket —
    /// finalizing is the operator's call.
    pub fn handle_ingest(
        &self,
        slug: &str,
        station: i64,
        st: &Value,
    ) -> Result<Value, (Value, u16)> {
        if !st.is_object() {
            return Err((json!({"error": "Missing set."}), 400));
        }
        let rec = {
            let mut s = self.state.lock().unwrap();
            let (key, mut rec) = self.record_for(&s, slug, station, st, "recorded");
            if rec["status"] != *"reported" && get_default_true(&rec, "reportable") {
                rec["status"] = json!(if truthy(rec.get("matchedStartggSetId")) {
                    "matched"
                } else {
                    "recorded"
                });
            }
            set_bucket(&mut s, slug).insert(key, rec.clone());
            self.touch(&mut s);
            rec
        };
        (self.log)(&format!(
            "ingested set {} from station {}",
            py_str(st.get("setId").unwrap_or(&Value::Null)),
            station
        ));
        Ok(json!({"ok": true, "record": rec}))
    }

    // -- operator-facing ----------------------------------------------------
    pub fn event_view(&self, slug: &str) -> Value {
        let mut s = self.state.lock().unwrap();
        let mut sets: Vec<Value> = set_bucket(&mut s, slug).values().cloned().collect();
        sets.sort_by_key(ingested);
        let stations = s.stations.get(slug).cloned().unwrap_or_else(|| json!({}));
        json!({"slug": slug, "stations": stations, "sets": sets})
    }

    /// Everything the operator UI renders, newest set first.
    pub fn snapshot(&self) -> Value {
        let s = self.state.lock().unwrap();
        snapshot_of(&s)
    }

    pub fn get_set(&self, slug: &str, station: i64, set_id: &Value) -> Option<Value> {
        let mut s = self.state.lock().unwrap();
        let key = sid(station, set_id);
        set_bucket(&mut s, slug).get(&key).cloned()
    }

    /// Re-ask start.gg what's at this station and re-evaluate the record.
    ///
    /// A set finished before the TO pressed Start Match is correctly refused at
    /// the time; once they do start it, nothing else would revisit that record
    /// (finished sets get no further station updates), so the operator's next
    /// Report re-checks instead of being stuck.
    pub fn rebind(&self, slug: &str, station: i64, set_id: &Value) -> Option<Value> {
        let rec = self.get_set(slug, station, set_id)?;
        let set_v = rec.get("set").cloned().unwrap_or_else(|| json!({}));
        if !is_reportable(mode_of(&set_v).as_deref()) {
            return Some(rec);
        }
        let sg = match self.startgg.station_set(slug, station, 0.0) {
            Ok(v) => v,
            Err(e) => {
                (self.log)(&format!("re-check failed: {e}"));
                return Some(rec);
            }
        };
        if !matching::set_started(Some(&sg)) {
            return Some(rec);
        }
        let updated = {
            let mut s = self.state.lock().unwrap();
            // Refresh the station's cached binding too, so later updates agree.
            {
                let slug_map = s
                    .stations
                    .entry(slug.to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("stations bucket is a JSON object");
                let stn = slug_map
                    .entry(station.to_string())
                    .or_insert_with(|| json!({}));
                stn["startgg"] = sg.clone();
            }
            let wm = matching::match_winner(&set_v, sg.get("entrants"), Some(&s.tag_map));
            let key = sid(station, set_id);
            let mut updated = rec.clone();
            {
                let apply = |r: &mut Value| {
                    r["matchedStartggSetId"] = sg.get("setId").cloned().unwrap_or(Value::Null);
                    r["fullRoundText"] = sg.get("fullRoundText").cloned().unwrap_or(Value::Null);
                    r["entrants"] = sg.get("entrants").cloned().unwrap_or(Value::Null);
                    r["startggState"] = sg.get("state").cloned().unwrap_or(Value::Null);
                    r["reportable"] = json!(true);
                    r["notReportableReason"] = Value::Null;
                    if r["status"] != *"reported" {
                        r["status"] = json!("matched");
                    }
                    r["candidateWinnerEntrantId"] = wm
                        .candidate_winner_entrant_id
                        .clone()
                        .unwrap_or(Value::Null);
                    r["confidence"] = json!(wm.confidence);
                };
                match set_bucket(&mut s, slug).get_mut(&key) {
                    Some(r) => {
                        apply(r);
                        updated = r.clone();
                    }
                    None => apply(&mut updated),
                }
            }
            self.touch(&mut s);
            updated
        };
        (self.log)(&format!(
            "set {} re-bound: match is now started on start.gg",
            py_str(set_id)
        ));
        Some(updated)
    }

    /// Advance the bracket. Operator action only.
    pub fn do_report(
        &self,
        slug: &str,
        station: i64,
        set_id: &Value,
        winner_entrant_id: &Value,
    ) -> Result<Value, (Value, u16)> {
        let mut rec = match self.get_set(slug, station, set_id) {
            Some(r) => r,
            None => return Err((json!({"error": "Set not found."}), 404)),
        };
        // The TO may have pressed Start Match after this set finished.
        if !get_default_true(&rec, "reportable") {
            if let Some(r) = self.rebind(slug, station, set_id) {
                rec = r;
            }
        }
        if !get_default_true(&rec, "reportable") {
            let why = match rec.get("notReportableReason") {
                Some(v) if truthy(Some(v)) => py_str(v),
                _ => "not a tournament set".to_string(),
            };
            return Err((json!({"error": format!("Not reportable: {}.", why)}), 409));
        }
        if !truthy(rec.get("matchedStartggSetId")) {
            return Err((
                json!({"error": "This set is not matched to a start.gg set."}),
                409,
            ));
        }
        if !self.startgg.enabled() {
            return Err((
                json!({"error": "No start.gg token configured on the hub."}),
                501,
            ));
        }
        // Python: winner_entrant_id = str(winner_entrant_id or '')
        let wid = if truthy(Some(winner_entrant_id)) {
            py_str(winner_entrant_id)
        } else {
            String::new()
        };
        if wid.is_empty() {
            return Err((json!({"error": "Missing winnerEntrantId."}), 400));
        }
        let ents = rec.get("entrants").cloned().unwrap_or_else(|| json!([]));
        let ents_arr = ents.as_array().cloned().unwrap_or_default();
        if !ents_arr.is_empty()
            && !ents_arr
                .iter()
                .any(|e| py_str(e.get("id").unwrap_or(&Value::Null)) == wid)
        {
            return Err((
                json!({"error": "winnerEntrantId is not one of this set's entrants."}),
                400,
            ));
        }

        let tag_map = self.state.lock().unwrap().tag_map.clone();
        // Python wrapped this block in try/except Exception -> winner-only.
        let mut game_data: Option<Value> = None;
        if let Some(slot_map) =
            matching::map_slots_to_entrants(&rec, Some(&json!(wid.clone())), Some(&tag_map))
        {
            if let Ok(cmap_v) = self.startgg.character_map(slug) {
                let cmap = char_map_of(&cmap_v);
                let games_v = rec
                    .get("set")
                    .and_then(|st| st.get("games"))
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                let gd = matching::game_data_from_games(&games_v, &slot_map, &cmap);
                let complete = gd
                    .as_array()
                    .map(|a| !a.is_empty() && a.iter().all(|g| truthy(g.get("winnerId"))))
                    .unwrap_or(false);
                if complete {
                    game_data = Some(gd);
                }
            }
        }
        if let Err(e) = self.startgg.report_set(
            &rec["matchedStartggSetId"],
            &json!(wid.clone()),
            game_data.as_ref(),
        ) {
            return Err((
                json!({"error": format!("start.gg report failed: {}", e)}),
                502,
            ));
        }
        let games_reported = game_data
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        {
            let mut s = self.state.lock().unwrap();
            let key = sid(station, set_id);
            {
                let apply = |r: &mut Value| {
                    r["status"] = json!("reported");
                    r["reportedAt"] = json!(now_sec());
                    r["reportedWinnerEntrantId"] = json!(wid.clone());
                    r["reportedGames"] = json!(games_reported);
                    r["reportedBy"] = json!("operator");
                };
                match set_bucket(&mut s, slug).get_mut(&key) {
                    Some(r) => {
                        apply(r);
                        rec = r.clone();
                    }
                    None => apply(&mut rec),
                }
            }
            self.touch(&mut s);
        }
        (self.log)(&format!(
            "reported set {} (station {}) to start.gg",
            py_str(set_id),
            station
        ));
        Ok(json!({"ok": true, "record": rec, "gamesReported": games_reported}))
    }

    /// Remember which save tag belongs to which start.gg entrant.
    ///
    /// The station can only ever guess: in-game tags don't have to match
    /// start.gg tags. Once the operator corrects a set, that pairing is a fact
    /// — record it so the next set between the same people maps right without
    /// another correction. Kept in its own file so a hand-written players.json
    /// is never rewritten.
    fn learn_aliases(&self, s: &mut HubState, rec: &Value) {
        let smap = match matching::map_slots_to_entrants(rec, None, Some(&s.tag_map)) {
            Some(m) => m,
            None => return,
        };
        let mut by_id: HashMap<String, Value> = HashMap::new();
        let ents = rec.get("entrants").cloned().unwrap_or_else(|| json!([]));
        if let Some(arr) = ents.as_array() {
            for e in arr {
                by_id.insert(
                    py_str(e.get("id").unwrap_or(&Value::Null)),
                    e.get("name").cloned().unwrap_or(Value::Null),
                );
            }
        }
        let players = rec
            .get("set")
            .and_then(|st| st.get("players"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let mut learned = false;
        if let Some(parr) = players.as_array() {
            for p in parr {
                let gg = p
                    .get("slot")
                    .and_then(|v| v.as_i64())
                    .and_then(|sl| smap.get(&sl))
                    .and_then(|eid| by_id.get(eid))
                    .cloned();
                let name_v = p.get("name").cloned().unwrap_or(Value::Null);
                let key = matching::norm(&name_v);
                if let Some(gg_v) = gg {
                    if truthy(Some(&gg_v)) && !key.is_empty() {
                        let gg_s = py_str(&gg_v);
                        if s.tag_map.get(&key) != Some(&gg_s) {
                            s.tag_map.insert(key, gg_s);
                            s.learned.insert(py_str(&name_v), gg_v.clone());
                            learned = true;
                        }
                    }
                }
            }
        }
        if learned {
            if let Some(lp) = &self.learned_path {
                let body = serde_json::to_string_pretty(&Value::Object(s.learned.clone()))
                    .unwrap_or_else(|_| "{}".to_string());
                let tmp = format!("{lp}.tmp");
                if let Err(e) = fs::write(&tmp, body).and_then(|_| fs::rename(&tmp, lp)) {
                    (self.log)(&format!("could not save learned tags: {e}"));
                }
            }
            let pairs: Vec<String> = s
                .learned
                .iter()
                .map(|(k, v)| format!("{} -> {}", k, py_str(v)))
                .collect();
            (self.log)(&format!("learned tag mapping: {}", pairs.join(", ")));
        }
    }

    /// Flip which in-game player maps to which start.gg entrant, re-push the
    /// corrected live score, and remember the pairing for future sets.
    pub fn do_swap(&self, slug: &str, station: i64, set_id: &Value) -> Result<Value, (Value, u16)> {
        let key = sid(station, set_id);
        let (rec, tag_map) = {
            let mut s = self.state.lock().unwrap();
            let mut rec = match set_bucket(&mut s, slug).get(&key).cloned() {
                Some(r) => r,
                None => return Err((json!({"error": "Set not found."}), 404)),
            };
            rec["swap"] = json!(!truthy(rec.get("swap")));
            if truthy(rec.get("candidateWinnerEntrantId")) {
                let ents = rec.get("entrants").cloned().unwrap_or_else(|| json!([]));
                if let Some(arr) = ents.as_array().filter(|a| a.len() == 2) {
                    let cand_s = py_str(&rec["candidateWinnerEntrantId"]);
                    if let Some(other) = arr
                        .iter()
                        .find(|e| py_str(e.get("id").unwrap_or(&Value::Null)) != cand_s)
                    {
                        rec["candidateWinnerEntrantId"] =
                            other.get("id").cloned().unwrap_or(Value::Null);
                    }
                }
            }
            self.learn_aliases(&mut s, &rec);
            set_bucket(&mut s, slug).insert(key.clone(), rec.clone());
            self.touch(&mut s);
            let tm = s.tag_map.clone();
            (rec, tm)
        };
        let mut repushed = false;
        if self.startgg.enabled()
            && truthy(rec.get("matchedStartggSetId"))
            && get_default_true(&rec, "reportable")
        {
            // Python: try ... except StartggError -> log('swap re-push failed').
            let attempt = (|| -> Result<bool, StartggError> {
                if let Some(slot_map) = matching::map_slots_to_entrants(&rec, None, Some(&tag_map))
                {
                    let cmap = char_map_of(&self.startgg.character_map(slug)?);
                    let games_v = rec
                        .get("set")
                        .and_then(|st| st.get("games"))
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    let gd: Vec<Value> = matching::game_data_from_games(&games_v, &slot_map, &cmap)
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|g| truthy(g.get("winnerId")))
                        .collect();
                    if !gd.is_empty() {
                        self.startgg
                            .update_live(&rec["matchedStartggSetId"], &Value::Array(gd))?;
                        return Ok(true);
                    }
                }
                Ok(false)
            })();
            match attempt {
                Ok(v) => repushed = v,
                Err(e) => (self.log)(&format!("swap re-push failed: {e}")),
            }
        }
        Ok(json!({"ok": true, "swap": rec["swap"], "repushed": repushed, "record": rec}))
    }

    /// Drop a set from the operator's view. start.gg is never touched.
    pub fn do_delete(
        &self,
        slug: &str,
        station: i64,
        set_id: &Value,
    ) -> Result<Value, (Value, u16)> {
        {
            let mut s = self.state.lock().unwrap();
            let removed = set_bucket(&mut s, slug).remove(&sid(station, set_id));
            if removed.is_none() {
                return Err((json!({"error": "Set not found."}), 404));
            }
            self.touch(&mut s);
        }
        (self.log)(&format!(
            "deleted set {} (station {})",
            py_str(set_id),
            station
        ));
        Ok(json!({"ok": true}))
    }
}

// ---------------------------------------------------------------------------
// HTTP front end
// ---------------------------------------------------------------------------

const PREFIX: &str = "/matchlogger/";

fn hexval(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Percent-decoding as `urllib.parse.parse_qs` applies it ('+' is a space).
fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(a), Some(b)) = (hexval(bytes[i + 1]), hexval(bytes[i + 2])) {
                    out.push(a * 16 + b);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `(parse_qs(query).get(name) or [''])[0]` — parse_qs drops blank values, so
/// the first non-blank occurrence wins.
fn query_param(query: &str, name: &str) -> String {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if urldecode(k) == name {
            let dv = urldecode(v);
            if !dv.is_empty() {
                return dv;
            }
        }
    }
    String::new()
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unhandled panic".to_string()
    }
}

fn respond(req: tiny_http::Request, obj: Value, code: u16) {
    let body = obj.to_string();
    let mut resp = tiny_http::Response::from_data(body.into_bytes()).with_status_code(code);
    // The operator console may be served from anywhere on the LAN.
    for (k, v) in [
        ("Content-Type", "application/json"),
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Headers", "Content-Type"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
        ("Server", "RivalsHub/1.0"),
    ] {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

fn route_get(hub: &Hub, url: &str) -> (Value, u16) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if !path.starts_with(PREFIX) {
        return (json!({"error": "Not found."}), 404);
    }
    let op = &path[PREFIX.len()..];
    let slug = query_param(query, "slug");
    match op {
        "version" => (json!({"v": hub.version()}), 200),
        "event" => {
            if slug.is_empty() {
                (json!({"error": "Expected an event slug."}), 400)
            } else {
                (hub.event_view(&slug), 200)
            }
        }
        "health" => (json!({"ok": true, "startgg": hub.startgg.enabled()}), 200),
        _ => (json!({"error": "Unknown operation."}), 404),
    }
}

fn route_post(hub: &Hub, req: &mut tiny_http::Request, url: &str) -> (Value, u16) {
    if !url.starts_with(PREFIX) {
        return (json!({"error": "Not found."}), 404);
    }
    let op = url[PREFIX.len()..]
        .split('?')
        .next()
        .unwrap_or("")
        .to_string();
    let mut raw = Vec::new();
    if req.as_reader().read_to_end(&mut raw).is_err() {
        return (json!({"error": "Expected a JSON body."}), 400);
    }
    let body: Value = if raw.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(_) => return (json!({"error": "Expected a JSON body."}), 400),
        }
    };
    if !body.is_object() {
        return (json!({"error": "Expected a JSON object."}), 400);
    }

    let slug = match body.get("slug") {
        Some(v) if truthy(Some(v)) => py_str(v).trim().to_string(),
        _ => String::new(),
    };
    if slug.is_empty() {
        return (json!({"error": "Bad or missing event slug."}), 400);
    }
    // One shared key for stations and operator actions, like the broker.
    let supplied = if matches!(op.as_str(), "current" | "live" | "ingest") {
        body.get("key")
    } else {
        body.get("passcode")
    };
    if !hub.check_key(supplied) {
        return (json!({"error": "Bad key."}), 401);
    }

    let station = py_int(body.get("station"));
    if station.is_none()
        && matches!(
            op.as_str(),
            "current" | "live" | "ingest" | "report" | "swap" | "delete"
        )
    {
        return (json!({"error": "Bad or missing station."}), 400);
    }
    let station = station.unwrap_or(0);

    let null = Value::Null;
    let dispatch = || -> Option<Result<Value, (Value, u16)>> {
        match op.as_str() {
            "current" => Some(hub.handle_current(&slug, station, body.get("current"))),
            "live" => Some(hub.handle_live(&slug, station, body.get("set").unwrap_or(&null))),
            "ingest" => Some(hub.handle_ingest(&slug, station, body.get("set").unwrap_or(&null))),
            "report" => Some(hub.do_report(
                &slug,
                station,
                body.get("setId").unwrap_or(&null),
                body.get("winnerEntrantId").unwrap_or(&null),
            )),
            "swap" => Some(hub.do_swap(&slug, station, body.get("setId").unwrap_or(&null))),
            "delete" => Some(hub.do_delete(&slug, station, body.get("setId").unwrap_or(&null))),
            _ => None,
        }
    };
    match catch_unwind(AssertUnwindSafe(dispatch)) {
        Ok(Some(Ok(v))) => (v, 200),
        Ok(Some(Err((v, c)))) => (v, c),
        Ok(None) => (json!({"error": "Unknown operation."}), 404),
        Err(p) => {
            // never kill the server
            let msg = panic_message(&*p);
            hub.log(&format!("hub error on /{op}: {msg}"));
            (json!({"error": format!("Hub error: {}", msg)}), 500)
        }
    }
}

fn handle_request(hub: &Hub, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let (obj, code) = match req.method() {
        tiny_http::Method::Options => (json!({}), 204),
        tiny_http::Method::Get => route_get(hub, &url),
        tiny_http::Method::Post => route_post(hub, &mut req, &url),
        // BaseHTTPRequestHandler answers 501 for unimplemented methods.
        _ => (json!({"error": "Unsupported method."}), 501),
    };
    respond(req, obj, code);
}

/// Runs a [`Hub`] over HTTP on the LAN, in a background thread.
pub struct HubServer {
    hub: Arc<Hub>,
    pub port: u16,
    pub bind: String,
    srv: Option<Arc<tiny_http::Server>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HubServer {
    pub fn new(hub: Arc<Hub>, port: u16, bind: &str) -> HubServer {
        HubServer {
            hub,
            port,
            bind: bind.to_string(),
            srv: None,
            thread: None,
        }
    }

    pub fn running(&self) -> bool {
        self.srv.is_some()
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}", lan_ip(), self.port)
    }

    pub fn start(&mut self) -> Result<String, String> {
        if self.srv.is_some() {
            return Ok(self.url());
        }
        let server = tiny_http::Server::http(format!("{}:{}", self.bind, self.port))
            .map_err(|e| e.to_string())?;
        let server = Arc::new(server);
        let hub = self.hub.clone();
        let srv = server.clone();
        // Thread-per-request, mirroring ThreadingHTTPServer's daemon threads.
        self.thread = Some(thread::spawn(move || {
            for request in srv.incoming_requests() {
                let hub = hub.clone();
                thread::spawn(move || handle_request(&hub, request));
            }
        }));
        self.srv = Some(server);
        self.hub.log(&format!(
            "hub listening on {} (stations point here)",
            self.url()
        ));
        Ok(self.url())
    }

    pub fn stop(&mut self) {
        if let Some(srv) = self.srv.take() {
            srv.unblock();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — a 1:1 port of test_hub.py. The Python suite drove a real HubServer
// with the real station_sender; here reqwest::blocking plays the station,
// POSTing the exact bodies the sender sends, and FakeStartgg stands in for
// the real client so nothing touches a live bracket.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};

    const SLUG: &str = "tournament/the-hangout-4-1/event/rivals-of-aether-ii-singles";
    const KEY: &str = "thehangout2026!";

    /// The real set, as the station writes it (REAL_SET in test_hub.py).
    fn real_set() -> Value {
        json!({
            "setId": "20260724_075508", "complete": true, "matchCount": 2,
            "winnerSlot": 0, "winnerName": "JUGZ!", "winnerCharacter": "Orc",
            "startEpoch": 1784879708i64, "endEpoch": 1784879970i64,
            "players": [
                {"slot": 0, "name": "JUGZ!", "character": "Orc", "wins": 2},
                {"slot": 1, "name": "KIM", "character": "Gal", "wins": 0},
            ],
            "matches": [
                {"index": 1, "players": [
                    {"slot": 0, "name": "JUGZ!", "character": "Orc", "wins": 1},
                    {"slot": 1, "name": "KIM", "character": "Gal", "wins": 0}]},
                {"index": 2, "players": [
                    {"slot": 0, "name": "JUGZ!", "character": "Orc", "wins": 2},
                    {"slot": 1, "name": "KIM", "character": "Gal", "wins": 0}]},
            ],
        })
    }

    fn entrants() -> Value {
        json!([{"id": 24186345, "name": "jugeeya"}, {"id": 24186347, "name": "Kimchi"}])
    }

    fn tags() -> HashMap<String, String> {
        matching::build_tag_map(Some(&json!({"JUGZ!": "jugeeya", "KIM": "Kimchi"})))
    }

    /// Copy of a set with overrides — Python's `dict(REAL_SET, k=v, ...)`.
    fn with(mut st: Value, overrides: &[(&str, Value)]) -> Value {
        for (k, v) in overrides {
            st[*k] = v.clone();
        }
        st
    }

    /// Stands in for the real client: records what would have been sent.
    struct FakeStartgg {
        enabled: bool,
        /// 2 = ongoing (TO pressed Start Match); 6 = called, not started
        state: Mutex<i64>,
        live_pushes: Mutex<Vec<(Value, Value)>>,
        reports: Mutex<Vec<(Value, Value, Option<Value>)>>,
    }

    impl FakeStartgg {
        fn new() -> Arc<FakeStartgg> {
            Arc::new(FakeStartgg {
                enabled: true,
                state: Mutex::new(2),
                live_pushes: Mutex::new(Vec::new()),
                reports: Mutex::new(Vec::new()),
            })
        }

        fn set_state(&self, st: i64) {
            *self.state.lock().unwrap() = st;
        }

        fn pushes(&self) -> Vec<(Value, Value)> {
            self.live_pushes.lock().unwrap().clone()
        }

        fn reports(&self) -> Vec<(Value, Value, Option<Value>)> {
            self.reports.lock().unwrap().clone()
        }
    }

    /// `h.startgg = FakeStartgg()` in Python: the hub owns a Box while the
    /// test keeps its own handle to inspect the recorded calls.
    struct Shared(Arc<FakeStartgg>);

    impl StartggApi for Shared {
        fn enabled(&self) -> bool {
            self.0.enabled
        }
        fn station_set(
            &self,
            _slug: &str,
            _station: i64,
            _max_age: f64,
        ) -> Result<Value, StartggError> {
            Ok(json!({
                "found": true, "setId": 105639152, "state": *self.0.state.lock().unwrap(),
                "fullRoundText": "Winners Round 1", "entrants": entrants(),
            }))
        }
        fn character_map(&self, _slug: &str) -> Result<Value, StartggError> {
            Ok(json!({"orcane": 41, "galvan": 42, "random": 99}))
        }
        fn update_live(&self, set_id: &Value, game_data: &Value) -> Result<(), StartggError> {
            self.0
                .live_pushes
                .lock()
                .unwrap()
                .push((set_id.clone(), game_data.clone()));
            Ok(())
        }
        fn report_set(
            &self,
            set_id: &Value,
            winner_entrant_id: &Value,
            game_data: Option<&Value>,
        ) -> Result<Value, StartggError> {
            self.0.reports.lock().unwrap().push((
                set_id.clone(),
                winner_entrant_id.clone(),
                game_data.cloned(),
            ));
            Ok(json!({}))
        }
    }

    static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

    fn tmpdir(prefix: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "{}_{}_{}",
            prefix,
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn path_str(p: &std::path::Path) -> String {
        p.to_string_lossy().into_owned()
    }

    /// The main flow of test_hub.py: a real HubServer on localhost, driven the
    /// way a station PC drives it (same POST bodies station_sender sends).
    #[test]
    fn end_to_end_over_http() {
        let workdir = tmpdir("hubtest");
        let state_path = path_str(&workdir.join("hub-state.json"));
        let fake = FakeStartgg::new();
        let mut h = Hub::new(
            Some(KEY),
            None,
            Some(tags()),
            None,
            Some(state_path.clone()),
            None,
            None,
            None,
        );
        h.startgg = Box::new(Shared(fake.clone()));
        let h = Arc::new(h);

        let port = free_port();
        let mut server = HubServer::new(h.clone(), port, "127.0.0.1");
        server.start().expect("hub server starts");
        let broker = format!("http://127.0.0.1:{port}");
        let client = reqwest::blocking::Client::new();
        let post = |path: &str, body: Value| {
            client
                .post(format!("{broker}{path}"))
                .json(&body)
                .send()
                .expect("request to the hub")
        };

        // ---- the station, pointed at the hub --------------------------------
        // heartbeat: a set is starting at station 1
        let r = post(
            "/matchlogger/current",
            json!({"slug": SLUG, "station": 1, "key": KEY,
                   "current": {"state": "set_start", "epoch": 1784879708i64,
                               "setId": "20260724_075508"}}),
        );
        assert!(r.status().is_success());
        let snap = h.snapshot();
        assert!(
            !snap["stations"][SLUG]["1"].is_null(),
            "station heartbeat reached the hub"
        );
        assert_eq!(
            snap["stations"][SLUG]["1"]["startgg"]["setId"],
            json!(105639152),
            "set_start pre-bound the start.gg set + entrants"
        );

        // live: a running set
        let live_body = with(
            real_set(),
            &[("complete", json!(false)), ("matchCount", json!(2))],
        );
        post(
            "/matchlogger/live",
            json!({"slug": SLUG, "station": 1, "key": KEY, "set": live_body}),
        );
        let pushes = fake.pushes();
        assert_eq!(pushes.len(), 1, "live update pushed to start.gg");
        let (pushed_id, pushed_games) = pushes.last().unwrap().clone();
        assert_eq!(
            pushed_id,
            json!(105639152),
            "pushed against the matched start.gg set"
        );
        let games = pushed_games.as_array().unwrap();
        assert!(
            games.len() == 2 && games.iter().all(|g| truthy(g.get("winnerId"))),
            "BOTH games pushed with a winnerId (2-0 reports as 2-0)"
        );
        assert!(
            games.iter().all(|g| g.get("selections").is_some()),
            "character selections included"
        );
        assert_eq!(
            fake.reports().len(),
            0,
            "live push never advanced the bracket (no reportBracketSet call)"
        );

        // ingest: the finished set
        post(
            "/matchlogger/ingest",
            json!({"slug": SLUG, "station": 1, "key": KEY, "set": real_set()}),
        );
        let view = h.event_view(SLUG);
        assert_eq!(
            view["sets"].as_array().unwrap().len(),
            1,
            "hub holds exactly one set record"
        );
        let rec = view["sets"][0].clone();
        assert!(
            rec["status"] == *"matched" || rec["status"] == *"live",
            "set is matched to start.gg  [{}]",
            rec["status"]
        );
        assert_eq!(
            py_str(&rec["candidateWinnerEntrantId"]),
            "24186345",
            "candidate winner resolved to jugeeya via players.json"
        );
        assert_eq!(rec["confidence"], json!("high"), "confidence high");
        assert_eq!(
            fake.reports().len(),
            0,
            "ingest did NOT report to start.gg (finalizing stays manual)"
        );

        // ---- version counter -------------------------------------------------
        let v1 = h.version();
        h.handle_current(SLUG, 1, Some(&json!({"state": "idle"})))
            .unwrap();
        assert!(
            h.version() > v1,
            "version counter bumps on change (cheap console polling)"
        );

        // ---- operator actions ------------------------------------------------
        let set_id = json!("20260724_075508");
        let res = h.do_swap(SLUG, 1, &set_id).unwrap();
        assert_eq!(res["swap"], json!(true), "swap toggles the mapping");
        assert_eq!(
            py_str(&h.get_set(SLUG, 1, &set_id).unwrap()["candidateWinnerEntrantId"]),
            "24186347",
            "swap flips the candidate winner to Kimchi"
        );
        assert_eq!(
            res["repushed"],
            json!(true),
            "swap immediately re-pushed the corrected live score"
        );
        h.do_swap(SLUG, 1, &set_id).unwrap(); // back

        let rep = h.do_report(SLUG, 1, &set_id, &json!(24186345)).unwrap();
        assert_eq!(rep["ok"], json!(true), "operator report succeeded");
        let reports = fake.reports();
        assert_eq!(reports.len(), 1, "reportBracketSet called exactly once");
        assert_eq!(
            reports[0].1,
            json!("24186345"),
            "reported winner is jugeeya"
        );
        assert_eq!(
            reports[0]
                .2
                .as_ref()
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0),
            2,
            "final report carried both games"
        );
        assert_eq!(
            h.get_set(SLUG, 1, &set_id).unwrap()["status"],
            json!("reported"),
            "status -> reported"
        );

        // ---- delete ------------------------------------------------------------
        assert_eq!(
            h.do_delete(SLUG, 1, &set_id).unwrap()["ok"],
            json!(true),
            "delete removes the set"
        );
        assert_eq!(
            h.event_view(SLUG)["sets"].as_array().unwrap().len(),
            0,
            "set is gone from the view"
        );
        assert_eq!(fake.reports().len(), 1, "delete never touched start.gg");

        // ---- persistence ---------------------------------------------------------
        h.handle_ingest(SLUG, 3, &real_set()).unwrap();
        let h3 = Hub::new(
            Some(KEY),
            None,
            Some(tags()),
            None,
            Some(state_path),
            None,
            None,
            None,
        );
        assert_eq!(
            h3.event_view(SLUG)["sets"].as_array().unwrap().len(),
            1,
            "state survives a hub restart"
        );

        // ---- key gate --------------------------------------------------------
        assert!(
            h.check_key(Some(&json!(KEY))) && !h.check_key(Some(&json!("wrong"))),
            "shared key gate works"
        );

        // ---- online / ranked games are logged but never reported ---------------
        let before = fake.pushes().len();
        let online = with(
            real_set(),
            &[("setId", json!("ONLINE1")), ("mode", json!("ONLINE"))],
        );
        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        let res_live = h
            .handle_live(
                SLUG,
                1,
                &with(online.clone(), &[("complete", json!(false))]),
            )
            .unwrap();
        assert_eq!(
            fake.pushes().len(),
            before,
            "an ONLINE set is NOT pushed to start.gg  [{}]",
            res_live["reason"]
        );
        assert!(
            res_live["reason"].as_str().unwrap_or("").contains("online"),
            "reason names the mode"
        );
        h.handle_ingest(SLUG, 1, &online).unwrap();
        let orec = h.get_set(SLUG, 1, &json!("ONLINE1"));
        assert!(
            orec.is_some(),
            "the online set is still recorded (visible in the console)"
        );
        let orec = orec.unwrap();
        assert_eq!(
            orec["reportable"],
            json!(false),
            "record flagged not reportable"
        );
        assert_eq!(
            orec["status"],
            json!("online"),
            "status shows the mode  [{}]",
            orec["status"]
        );
        assert!(
            orec["matchedStartggSetId"].is_null(),
            "online set is not bound to the station's bracket set"
        );
        let rep_o = h.do_report(SLUG, 1, &json!("ONLINE1"), &json!(24186345));
        assert_eq!(
            rep_o.unwrap_err().1,
            409,
            "reporting an online set is refused (409)"
        );
        assert_eq!(fake.reports().len(), 1, "no extra start.gg report happened");

        let ranked = with(
            real_set(),
            &[("setId", json!("RANKED1")), ("mode", json!("RANKED"))],
        );
        h.handle_ingest(SLUG, 1, &ranked).unwrap();
        assert_eq!(
            h.get_set(SLUG, 1, &json!("RANKED1")).unwrap()["status"],
            json!("ranked"),
            "ranked labelled too"
        );

        // a LOCAL set is unaffected by the new gate
        let local = with(
            real_set(),
            &[("setId", json!("LOCAL1")), ("mode", json!("LOCAL"))],
        );
        h.handle_ingest(SLUG, 1, &local).unwrap();
        let lrec = h.get_set(SLUG, 1, &json!("LOCAL1")).unwrap();
        assert!(
            lrec["reportable"] == json!(true) && lrec["matchedStartggSetId"] == json!(105639152),
            "LOCAL sets still match and stay reportable"
        );

        // a station with the wrong key is rejected
        let r = post(
            "/matchlogger/current",
            json!({"slug": SLUG, "station": 9, "key": "wrong-key",
                   "current": {"state": "idle"}}),
        );
        assert_eq!(
            r.status().as_u16(),
            401,
            "a station with the wrong key is rejected"
        );
        assert_eq!(r.json::<Value>().unwrap()["error"], json!("Bad key."));

        server.stop();
        let _ = fs::remove_dir_all(&workdir);
    }

    // ---- report with no token degrades honestly --------------------------------
    #[test]
    fn report_with_no_token_degrades_honestly() {
        let h2 = Hub::new(None, None, Some(tags()), None, None, None, None, None);
        h2.handle_ingest(SLUG, 2, &real_set()).unwrap();
        let res2 = h2.do_report(SLUG, 2, &json!("20260724_075508"), &json!(1));
        let (_, code) = res2.expect_err("no token / unmatched must error");
        assert!(
            code == 409 || code == 501,
            "no token / unmatched -> honest error, not a silent success  [{code}]"
        );
    }

    // ---- safeguard: nothing happens until the TO presses Start Match -----------
    #[test]
    fn nothing_reported_until_to_presses_start_match() {
        let workdir = tmpdir("hubtest_h4");
        let called = FakeStartgg::new();
        called.set_state(6); // called to the station, but NOT started
        let mut h4 = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h4.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h4.startgg = Box::new(Shared(called.clone()));

        h4.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        let ns_live = with(
            real_set(),
            &[
                ("setId", json!("NS")),
                ("mode", json!("LOCAL")),
                ("complete", json!(false)),
            ],
        );
        let res_ns = h4.handle_live(SLUG, 1, &ns_live).unwrap();
        assert_eq!(
            called.pushes().len(),
            0,
            "a called-but-not-started match is NOT pushed  [{}]",
            res_ns["reason"]
        );
        assert!(
            res_ns["reason"]
                .as_str()
                .unwrap_or("")
                .contains("not started"),
            "reason says the match isn't started"
        );
        h4.handle_ingest(
            SLUG,
            1,
            &with(
                real_set(),
                &[("setId", json!("NS")), ("mode", json!("LOCAL"))],
            ),
        )
        .unwrap();
        let nrec = h4.get_set(SLUG, 1, &json!("NS")).unwrap();
        assert_eq!(
            nrec["reportable"],
            json!(false),
            "not-started set is not reportable"
        );
        assert!(
            nrec["matchedStartggSetId"].is_null(),
            "not-started set isn't bound to the bracket set"
        );
        assert_eq!(
            nrec["status"],
            json!("waiting for start"),
            "status 'waiting for start'  [{}]",
            nrec["status"]
        );
        let rep_ns = h4.do_report(SLUG, 1, &json!("NS"), &json!(24186345));
        assert_eq!(
            rep_ns.unwrap_err().1,
            409,
            "reporting a not-started match is refused"
        );
        assert_eq!(
            called.reports().len(),
            0,
            "nothing was reported to start.gg"
        );

        // ...and once the TO does start it, Report re-checks and goes through
        called.set_state(2);
        let rep_ok = h4
            .do_report(SLUG, 1, &json!("NS"), &json!(24186345))
            .expect("after Start Match, the same set reports fine");
        assert_eq!(
            rep_ok["ok"],
            json!(true),
            "after Start Match, the same set reports fine  [{rep_ok}]"
        );
        assert_eq!(
            called.reports().len(),
            1,
            "exactly one report reached start.gg"
        );
        let _ = fs::remove_dir_all(&workdir);
    }

    // We must never start a match ourselves. Python monkeypatched the client's
    // _gql and inspected the sent query; the Rust client's transport isn't
    // injectable, so assert the same property against the one mutation
    // update_live() sends (its GraphQL const in startgg.rs).
    #[test]
    fn live_push_never_starts_a_match() {
        let src = include_str!("startgg.rs");
        let start = src
            .find("const UPDATE_LIVE_MUTATION")
            .expect("update_live mutation const present");
        let rest = &src[start..];
        let stmt = &rest[..rest.find(';').expect("const statement ends")];
        assert!(
            stmt.contains("updateBracketSet"),
            "the live push is updateBracketSet"
        );
        assert!(
            !stmt.contains("markSetInProgress"),
            "it's updateBracketSet only — we never start a match ourselves"
        );
        assert!(
            !stmt.contains("reportBracketSet"),
            "the live push never reports a winner"
        );
    }

    // ---- Switch players: the correction is remembered ----------------------------
    #[test]
    fn switch_players_correction_is_remembered() {
        let workdir = tmpdir("hubtest_h5");
        let learned_path = path_str(&workdir.join("learned5.json"));
        let fake = FakeStartgg::new();
        let mut h5 = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h5.json"))),
            None,
            None,
            Some(learned_path.clone()),
        );
        h5.startgg = Box::new(Shared(fake.clone()));

        h5.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h5.handle_ingest(
            SLUG,
            1,
            &with(
                real_set(),
                &[("setId", json!("SW")), ("mode", json!("LOCAL"))],
            ),
        )
        .unwrap();
        let before_map = h5.tag_map();
        h5.do_swap(SLUG, 1, &json!("SW")).unwrap();
        assert_ne!(
            h5.tag_map(),
            before_map,
            "switching players updates the tag map"
        );
        assert_eq!(
            h5.tag_map().get("jugz"),
            Some(&"Kimchi".to_string()),
            "JUGZ! now maps to Kimchi  [{:?}]",
            h5.tag_map().get("jugz")
        );
        assert!(
            Path::new(&learned_path).exists(),
            "correction persisted to disk"
        );
        let h6 = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            None,
            None,
            None,
            Some(learned_path),
        );
        assert_eq!(
            h6.tag_map().get("jugz"),
            Some(&"Kimchi".to_string()),
            "a restarted hub still knows the correction"
        );
        let _ = fs::remove_dir_all(&workdir);
    }

    // ---- tag database precedence: learned > players.json > tag database ---------
    #[test]
    fn tagdb_is_overridden_by_players_json_and_by_learned() {
        let tagdb_map: HashMap<String, String> = [
            ("jugz".to_string(), "someone-else".to_string()), // players.json disagrees
            ("newtag".to_string(), "brandnew".to_string()),   // nobody else knows this one
        ]
        .into_iter()
        .collect();

        // players.json (tags()) overrides the tag database for a tag both know,
        // and the tag database still fills in one players.json never mentions.
        let h = Hub::new(
            None,
            None,
            Some(tags()),
            Some(tagdb_map.clone()),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            h.tag_map().get("jugz"),
            Some(&"jugeeya".to_string()),
            "players.json overrides the tag database for a tag both know"
        );
        assert_eq!(
            h.tag_map().get("newtag"),
            Some(&"brandnew".to_string()),
            "the tag database fills in a tag nobody else knows"
        );

        // A learned correction (from a prior Switch Players) still wins over both.
        let workdir = tmpdir("hubtest_tagdb_precedence");
        let learned_path = path_str(&workdir.join("learned.json"));
        fs::write(&learned_path, r#"{"JUGZ!": "operator-correction"}"#).unwrap();
        let h2 = Hub::new(
            None,
            None,
            Some(tags()),
            Some(tagdb_map),
            None,
            None,
            None,
            Some(learned_path),
        );
        assert_eq!(
            h2.tag_map().get("jugz"),
            Some(&"operator-correction".to_string()),
            "a learned correction still wins over both players.json and the tag database"
        );
        let _ = fs::remove_dir_all(&workdir);
    }
}
