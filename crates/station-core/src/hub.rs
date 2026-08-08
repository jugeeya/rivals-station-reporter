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

/// Explicit app identifier reported on `/matchlogger/health` (alongside the
/// existing `Server: RivalsHub/…` header) so a LAN scanner can confirm it
/// found THIS app and not an unrelated service that happens to answer on the
/// same port. See `crate::discovery`, which probes every host on the local
/// /24 for exactly this.
pub const APP_ID: &str = "rivals-station-reporter-hub";

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

/// Which of start.gg's two similarly-named timestamps to trust for "when
/// this match started": `startedAt` is preferred (its name directly
/// parallels the markSetInProgress mutation and this app's own
/// STARTGG_STATE_ONGOING concept), falling back to `startAt` only when
/// `startedAt` is absent. This is a documented, reasoned-from-naming
/// inference, not a verified fact -- the test tournament used to build this
/// had no set in progress or completed, so which field actually populates
/// when a real match starts could not be confirmed live (see startgg.rs's
/// STATION_SET_QUERY doc). `sg` is the raw `Startgg::station_set` value (or
/// `Value::Null`/anything falsy, which yields `Value::Null` here too).
///
/// `pub(crate)` so `startgg.rs`'s `parse_available_sets` can reuse this
/// exact fallback for the Current Sets panel's "playing now" rows instead
/// of duplicating the logic.
pub(crate) fn preferred_started_at(sg: &Value) -> Value {
    match sg.get("startedAt") {
        Some(v) if truthy(Some(v)) => v.clone(),
        _ => match sg.get("startAt") {
            Some(v) if truthy(Some(v)) => v.clone(),
            _ => Value::Null,
        },
    }
}

/// Resolve a station's plain number to start.gg's opaque id, against a
/// fresh `available_sets` read -- shared by `Hub::do_start_match` and
/// `Hub::do_reassign_station`, neither of which ever trusts a raw id
/// supplied by the frontend, only a plain number cross-checked here.
/// Refuse to mutate a set from a bracket that hasn't been started yet.
///
/// start.gg reports such sets with placeholder ids (`preview_3396320_1_0`),
/// and both `assignStation` and `markSetInProgress` fail against them --
/// answering only "An unknown error has occurred", which told the operator
/// nothing and left no useful log line: the assignment simply never landed.
/// Catching it here yields an accurate message and skips a pointless round
/// trip. Reported from a real event whose bracket had not been started; the
/// same actions work once it has. See [`crate::startgg::is_preview_set_id`].
fn reject_preview_set(set_id: &Value) -> Result<(), (Value, u16)> {
    if crate::startgg::is_preview_set_id(set_id) {
        return Err((
            json!({"error": "This bracket hasn't been started on start.gg yet, so its                              sets don't exist there and can't be assigned or started.                              Start the bracket (or phase) on start.gg first."}),
            409,
        ));
    }
    Ok(())
}

fn resolve_station_id(data: &Value, num: i64) -> Result<Value, (Value, u16)> {
    let stations = data
        .get("stations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    stations
        .iter()
        .find(|s| s.get("number").and_then(|n| n.as_i64()) == Some(num))
        .map(|s| s.get("id").cloned().unwrap_or(Value::Null))
        .ok_or_else(|| {
            (
                json!({"error": format!("Station {} not found on start.gg.", num)}),
                404,
            )
        })
}

/// Same as [`resolve_station_id`], but for a stream: `Streams` has no small
/// human-facing integer the way `Stations` has `number`, so this resolves by
/// `streamName` instead -- still never trusting a raw id from the frontend.
fn resolve_stream_id(data: &Value, name: &str) -> Result<Value, (Value, u16)> {
    let streams = data
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    streams
        .iter()
        .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(name))
        .map(|s| s.get("id").cloned().unwrap_or(Value::Null))
        .ok_or_else(|| {
            (
                json!({"error": format!("Stream \"{}\" not found on start.gg.", name)}),
                404,
            )
        })
}

/// A set's assignment target: a physical station (by number) or a stream
/// setup (by name), resolved server-side either way. start.gg lets a set
/// carry both at once (on stream AND at a physical station), so callers get
/// a list of zero, one, or two of these rather than an either/or.
/// Shared by `Hub::do_start_match` and `Hub::do_reassign_destination`.
enum Destination {
    Station(i64),
    Stream(String),
}

impl Destination {
    fn from_parts(station_number: Option<i64>, stream_name: Option<String>) -> Vec<Destination> {
        let mut dests = Vec::new();
        if let Some(n) = station_number {
            dests.push(Destination::Station(n));
        }
        if let Some(s) = stream_name {
            dests.push(Destination::Stream(s));
        }
        dests
    }

    /// Whether a set (as `available_sets` normalizes it) is already assigned
    /// to this destination -- an unchanged destination is never reassigned.
    fn matches_current(&self, target: &Value) -> bool {
        match self {
            Destination::Station(n) => target.get("station").and_then(|v| v.as_i64()) == Some(*n),
            Destination::Stream(name) => {
                target.get("stream").and_then(|v| v.as_str()) == Some(name.as_str())
            }
        }
    }
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
///
/// Scoped to `event_slug`, the event this hub is configured for. Both maps
/// are keyed by slug and `hub-state.json` is never pruned, so without this an
/// install reused across events shows every set it has ever seen mixed into
/// the current bracket's console. `stations` additionally has to be flattened
/// out of its slug bucket: the UI keys it by station number, so handing it
/// the raw `{slug: {station: rec}}` map made it render slugs where station
/// numbers belong.
///
/// A hub with no slug configured (`None`) is left unscoped rather than
/// filtered to nothing — it has nothing to scope *to*, and a
/// local-scoreboard-only hub still needs to show its sets.
/// The slot to entrant pairing as `[{slot, entrantId, entrantName}]`, for the
/// console to render per player. `Null` when there is nothing to pair against
/// (no bracket set bound, or not two of each), which the UI shows as unknown
/// rather than inventing a guess.
fn slot_entrants(
    summary: &Value,
    entrants: Option<&Value>,
    tag_map: &HashMap<String, String>,
    swap: bool,
) -> Value {
    let Some(entrants) = entrants.filter(|e| truthy(Some(e))) else {
        return Value::Null;
    };
    // `swap` must ride on the probe: map_slots_to_entrants inverts on it, and
    // this stored view is what the console renders — an operator's swap that
    // flips the REAL report mapping but not the displayed one shows the
    // backwards pairing as if the correction never happened.
    let probe = json!({ "set": summary, "entrants": entrants, "swap": swap });
    let Some(map) = matching::map_slots_to_entrants(&probe, None, Some(tag_map)) else {
        return Value::Null;
    };
    let by_id = |id: &str| -> Value {
        entrants
            .as_array()
            .and_then(|es| {
                es.iter()
                    .find(|e| e["id"].to_string().trim_matches('"') == id)
            })
            .and_then(|e| e.get("name").cloned())
            .unwrap_or(Value::Null)
    };
    let mut rows: Vec<Value> = map
        .iter()
        .map(|(slot, eid)| json!({"slot": slot, "entrantId": eid, "entrantName": by_id(eid)}))
        .collect();
    rows.sort_by_key(|r| r["slot"].as_i64().unwrap_or(0));
    json!(rows)
}

fn snapshot_of(s: &HubState, event_slug: Option<&str>) -> Value {
    let buckets = |m: &Map<String, Value>| -> Vec<Value> {
        match event_slug {
            Some(slug) => m.get(slug).cloned().into_iter().collect(),
            None => m.values().cloned().collect(),
        }
    };

    let mut out: Vec<Value> = Vec::new();
    for bucket in buckets(&s.sets) {
        if let Some(b) = bucket.as_object() {
            out.extend(b.values().cloned());
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(ingested(b)));

    let mut stations = Map::new();
    for bucket in buckets(&s.stations) {
        if let Some(b) = bucket.as_object() {
            for (station, rec) in b {
                stations.insert(station.clone(), rec.clone());
            }
        }
    }

    json!({"version": s.version, "sets": out, "stations": Value::Object(stations)})
}

/// Python `Hub._set_bucket`: `self.sets.setdefault(slug, {})`.
///
/// A bucket that exists but ISN'T an object (a corrupt or hand-edited
/// `hub-state.json` — external input, never validated shape-deep by
/// `load_state`) is replaced with an empty one rather than `.expect`ed:
/// panicking here would 500 every hub request and unwind straight through
/// the Tauri command path, bricking the app over one bad byte on disk.
fn json_bucket<'a>(map: &'a mut Map<String, Value>, slug: &str) -> &'a mut Map<String, Value> {
    let entry = map
        .entry(slug.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("just ensured an object")
}

fn set_bucket<'a>(s: &'a mut HubState, slug: &str) -> &'a mut Map<String, Value> {
    json_bucket(&mut s.sets, slug)
}

/// Same shape guarantee for the per-slug stations map.
fn stations_bucket<'a>(s: &'a mut HubState, slug: &str) -> &'a mut Map<String, Value> {
    json_bucket(&mut s.stations, slug)
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
    /// The event this hub is configured for (the operator's `cfg.slug`),
    /// reported on `/matchlogger/health` so a LAN discovery scan can show
    /// which event it would connect a station to — separate from the
    /// per-request `slug` every station-facing method already takes (one
    /// `HubState` can hold buckets for several slugs, but in practice a hub
    /// only ever runs one event at a time). Not a `Hub::new` parameter: it's
    /// set once via `set_event_slug` right after construction, so adding it
    /// didn't require growing that constructor's already-long argument list
    /// or touching its call site in `hub_glue.rs`.
    event_slug: Mutex<Option<String>>,
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
            event_slug: Mutex::new(None),
        }
    }

    /// Emit to the hub's log callback (Python callers used `hub.log(...)`).
    pub fn log(&self, msg: &str) {
        (self.log)(msg);
    }

    /// Record which event this hub is running (the operator's `cfg.slug`),
    /// so `/matchlogger/health` — and therefore a LAN discovery scan — can
    /// report it. Blank/whitespace-only clears it, matching how `key` is
    /// normalized above.
    pub fn set_event_slug(&self, slug: &str) {
        let slug = slug.trim();
        *self.event_slug.lock().unwrap() = (!slug.is_empty()).then(|| slug.to_string());
    }

    /// The event this hub is configured for, if any (e.g. a local-scoreboard
    /// hub with no start.gg event has none).
    pub fn event_slug(&self) -> Option<String> {
        self.event_slug.lock().unwrap().clone()
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
            let snap = snapshot_of(s, self.event_slug().as_deref());
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

    /// The station record stored from the previous heartbeat/ingest, if any.
    /// Takes the lock only for the read, so callers can do network work
    /// between reading it and writing back.
    fn prev_station(&self, slug: &str, station: i64) -> Option<Value> {
        let s = self.state.lock().unwrap();
        s.stations
            .get(slug)
            .and_then(|m| m.get(station.to_string()))
            .cloned()
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
        let mut rec = json!({"station": station, "current": current, "updatedAt": now_sec()});
        // Previous record read under a short lock, network done with NO lock
        // held: bind_station_set is a start.gg round trip (up to the client's
        // 20s timeout), and holding the state lock across it would stall
        // every other station's heartbeat, the snapshot, and every operator
        // action behind one slow request.
        let prev = self.prev_station(slug, station);
        let state = &rec["current"]["state"];
        // "A set just started here" -> pre-bind the two entrants now, so the
        // eventual live/ingest can name a winner with far less ambiguity.
        // Two triggers: the legacy Python sender's explicit "set_start", or —
        // since this app's own set_machine only ever writes "set_open"/"idle"
        // — a "set_open" heartbeat whose setId this station wasn't already
        // on (or with no binding yet, so a failed lookup retries). Re-binding
        // on a NEW set is also what clears a stale binding left behind when
        // the previous set was rebound at report time: without it, the next
        // set's games would be live-pushed to the PREVIOUS set's bracket
        // entry.
        let prev_binding = prev
            .as_ref()
            .filter(|p| truthy(p.get("startgg")))
            .map(|p| p["startgg"].clone());
        let new_set_here = match &prev {
            Some(p) => p["current"]["setId"] != rec["current"]["setId"],
            None => true,
        };
        let rebind = state == &json!("set_start")
            || (state == &json!("set_open") && (new_set_here || prev_binding.is_none()));
        if rebind {
            let sg = self.bind_station_set(slug, station);
            if truthy(Some(&sg)) {
                rec["startgg"] = sg;
            }
            // A fresh lookup that finds nothing deliberately DROPS any old
            // binding: it belonged to the previous set at this station, and
            // "no start.gg set at this station" (kept off the bracket) is
            // strictly safer than pushing this set's games to that one.
        } else if let Some(b) = prev_binding {
            rec["startgg"] = b;
        }
        let mut s = self.state.lock().unwrap();
        stations_bucket(&mut s, slug).insert(station.to_string(), rec.clone());
        self.touch(&mut s);
        Ok(json!({"ok": true, "startgg": rec.get("startgg")}))
    }

    /// Which start.gg set a record from this station should bind to: the
    /// station's stored binding if it has one, else a fresh lookup. The lock
    /// is taken only for the read and released before any network — see
    /// `handle_current` for why the lookup must never run under it.
    fn station_binding(&self, slug: &str, station: i64) -> Value {
        if let Some(prev) = self.prev_station(slug, station) {
            if truthy(prev.get("startgg")) {
                return prev["startgg"].clone();
            }
        }
        self.bind_station_set(slug, station)
    }

    /// Build/refresh the stored record for a set coming off a station. `sg`
    /// is the station's start.gg binding, resolved by the caller (via
    /// `station_binding`) BEFORE taking the state lock.
    fn record_for(
        &self,
        s: &HubState,
        sg: Value,
        st: &Value,
        station: i64,
        slug: &str,
        status: &str,
    ) -> (String, Value) {
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
        // The operator's swap survives rebuilds, so the displayed mapping
        // below must be computed under it too.
        let swap = truthy(prev.get("swap"));
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
            "swap": json!(swap),
            "mode": st.get("mode"),
            "startggState": sg.get("state"),
            // start.gg's authoritative versions of the station's own
            // startEpoch guess and winsRequired guess -- see
            // preferred_started_at's doc for the startedAt/startAt caveat.
            // operatorFormat.ts prefers these over the local inference when
            // present, falling back to the station's own guess otherwise.
            "startggStartedAt": preferred_started_at(&sg),
            "startggTotalGames": sg.get("totalGames").cloned().unwrap_or(Value::Null),
            "reportable": reason.is_none(),
            "notReportableReason": reason.clone(),
            // Which in-game slot the hub believes is which bracket entrant.
            // The console cannot work this out for itself: its only other
            // handle is candidateWinnerEntrantId, which needs a set winner,
            // so a set still in progress would show every player as unknown
            // even though the pairing is already known here. Same call the
            // report path uses, so what the operator sees is what would be
            // sent.
            "slotEntrants": slot_entrants(&summary, sg.get("entrants"), &s.tag_map, swap),
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
        // Resolved before the lock: may hit start.gg (see handle_current).
        let sg = self.station_binding(slug, station);
        let (key, rec, tag_map) = {
            let mut s = self.state.lock().unwrap();
            let (key, mut rec) = self.record_for(&s, sg, st, station, slug, "live");
            // Don't clobber the mode label _record_for set for online/ranked.
            if rec["status"] != *"reported" && get_default_true(&rec, "reportable") {
                rec["status"] = json!("live");
            }
            set_bucket(&mut s, slug).insert(key.clone(), rec.clone());
            self.touch(&mut s);
            let tm = s.tag_map.clone();
            (key, rec, tm)
        };

        let (mut live, mut games) = (false, 0usize);
        let mut confirmed = false;
        let mut reason: Option<String> = None;
        if !get_default_true(&rec, "reportable") {
            let why = match rec.get("notReportableReason") {
                Some(v) if truthy(Some(v)) => py_str(v),
                _ => "not reportable".to_string(),
            };
            reason = Some(format!("{why}; logged, not reported"));
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
                        let set_id = rec["matchedStartggSetId"].clone();
                        match self.startgg.update_live(&set_id, &Value::Array(gd.clone())) {
                            Ok(()) => {
                                live = true;
                                games = gd.len();
                                // The mutation only echoes back {id, state}; it
                                // never confirms the game data itself landed, so
                                // read the set back and compare. A mismatch here
                                // isn't an error -- start.gg's write may simply
                                // not be visible yet -- it just means "not
                                // confirmed on THIS tick". The station keeps
                                // posting live updates every poll interval while
                                // the set is ongoing, so the next tick pushes
                                // (harmlessly re-sending the same data if nothing
                                // changed) and checks again; this loop across
                                // natural ticks IS the poll, rather than a
                                // separate busy-wait that would hold up this
                                // request or need an injected delay in tests.
                                confirmed = self
                                    .startgg
                                    .set_games(&set_id)
                                    .ok()
                                    .map(|remote| {
                                        matching::live_push_confirmed(
                                            &Value::Array(gd.clone()),
                                            &remote,
                                        )
                                    })
                                    .unwrap_or(false);
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
                "live (station {station}): pushed {games} game(s) to start.gg{}",
                if confirmed {
                    " (confirmed)"
                } else {
                    " (not yet confirmed)"
                }
            ));
        }

        // Patch the confirm result onto the record already stored above,
        // rather than computing it before the push (which hadn't happened
        // yet) -- this is the only place liveConfirmed is written, so a
        // record with no live push attempt simply never gets the field.
        {
            let mut s = self.state.lock().unwrap();
            if let Some(stored) = set_bucket(&mut s, slug).get_mut(&key) {
                stored["liveConfirmed"] = json!(confirmed);
            }
            self.touch(&mut s);
        }

        Ok(
            json!({"ok": true, "live": live, "games": games, "reason": reason, "confirmed": confirmed}),
        )
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
        // Resolved before the lock: may hit start.gg (see handle_current).
        let sg = self.station_binding(slug, station);
        let rec = {
            let mut s = self.state.lock().unwrap();
            let (key, mut rec) = self.record_for(&s, sg, st, station, slug, "recorded");
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
        // `event_slug` is a separate mutex from `state`, so taking it while
        // holding the state lock can't deadlock against `touch` doing the same.
        let slug = self.event_slug();
        let s = self.state.lock().unwrap();
        snapshot_of(&s, slug.as_deref())
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
                let stn = stations_bucket(&mut s, slug)
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
                    r["startggStartedAt"] = preferred_started_at(&sg);
                    r["startggTotalGames"] = sg.get("totalGames").cloned().unwrap_or(Value::Null);
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
        // A TO can finalize a set directly on start.gg's own page, entirely
        // bypassing this hub's Report button. `reportable` alone can't catch
        // that: it only ever gets set false by THIS hub's own bind/rebind
        // logic, so a set we already matched stays "reportable" forever as
        // far as our own state is concerned, no matter what happened to it
        // out from under us. Check the live state directly (bypassing
        // station_set's [1,2,6] filter, which can't tell "completed" apart
        // from "gone missing") right before the write neither request can
        // take back.
        if self.settle_if_reported_elsewhere(slug, station, set_id, &rec["matchedStartggSetId"]) {
            return Err((
                json!({"error": "This set was already reported directly on start.gg."}),
                409,
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

    /// Check whether `matched_startgg_set_id` shows completed directly on
    /// start.gg -- bypassing this hub's own Report button entirely -- and if
    /// so, mark the stored record settled (status/reportable/
    /// notReportableReason). Shared by `do_report` (checked right before an
    /// operator-triggered write, so a request neither side can take back
    /// never fires) and `sweep_reported_elsewhere` (a periodic background
    /// check over every awaiting-report record), so both apply the exact
    /// same "found it completed elsewhere" logic rather than duplicating it.
    ///
    /// Returns whether an external completion was found and applied. Any
    /// state other than completed, or a failed lookup, changes nothing and
    /// returns false -- only a CONFIRMED completion is grounds to stop
    /// treating a set as actionable; a transient lookup failure must not
    /// silently mark a genuinely fine set as settled.
    fn settle_if_reported_elsewhere(
        &self,
        slug: &str,
        station: i64,
        set_id: &Value,
        matched_startgg_set_id: &Value,
    ) -> bool {
        match self.startgg.set_state(matched_startgg_set_id) {
            Ok(v) if py_int(Some(&v)) == Some(matching::STARTGG_STATE_COMPLETED) => {
                let key = sid(station, set_id);
                let mut s = self.state.lock().unwrap();
                if let Some(stored) = set_bucket(&mut s, slug).get_mut(&key) {
                    stored["status"] = json!("already reported on start.gg");
                    stored["reportable"] = json!(false);
                    stored["notReportableReason"] = json!("already reported directly on start.gg");
                }
                self.touch(&mut s);
                true
            }
            _ => false,
        }
    }

    /// Safety net for a set someone finalized directly on start.gg's own
    /// page instead of clicking this hub's Report button: `do_report`'s own
    /// check (via `settle_if_reported_elsewhere`) only runs at the moment
    /// someone actually clicks Report, so a set sitting in "awaiting report"
    /// otherwise stays looking actionable indefinitely. This sweeps every
    /// such record in `slug`'s bucket and settles any that start.gg already
    /// shows completed.
    ///
    /// Scoped to `status == "matched"` with a non-null matchedStartggSetId --
    /// "live" sets are still being played (nothing to settle yet) and
    /// already-"reported"/settled records need no re-check, so neither is
    /// touched even if their remote state happened to read back as
    /// completed. Logs only when it actually settles something, so a quiet
    /// sweep (the common case -- the awaiting-report list is normally a
    /// handful of sets at most) doesn't spam the log on every tick.
    pub fn sweep_reported_elsewhere(&self, slug: &str) {
        // Collect the candidates first, then check each remotely -- avoids
        // holding the state lock across a blocking network call per record.
        let candidates: Vec<(i64, Value, Value)> = {
            let mut s = self.state.lock().unwrap();
            set_bucket(&mut s, slug)
                .values()
                .filter(|r| r["status"] == *"matched" && truthy(r.get("matchedStartggSetId")))
                .filter_map(|r| {
                    let station = r.get("station").and_then(|v| v.as_i64())?;
                    let set_id = r.get("id").cloned().unwrap_or(Value::Null);
                    let matched = r["matchedStartggSetId"].clone();
                    Some((station, set_id, matched))
                })
                .collect()
        };
        let mut settled = 0usize;
        for (station, set_id, matched) in &candidates {
            if self.settle_if_reported_elsewhere(slug, *station, set_id, matched) {
                settled += 1;
            }
        }
        if settled > 0 {
            (self.log)(&format!(
                "sweep: found {settled} set(s) already reported directly on start.gg"
            ));
        }
    }

    /// Sets for the operator's Current Sets panel -- both entrants
    /// determined, either playing now (state 2) or startable (state 1/6) --
    /// plus the event's stations. Read-only, like `event_view`.
    pub fn available_sets(&self, slug: &str) -> Result<Value, (Value, u16)> {
        if !self.startgg.enabled() {
            return Err((
                json!({"error": "No start.gg token configured on the hub."}),
                501,
            ));
        }
        self.startgg
            .available_sets(slug)
            .map_err(|e| (json!({"error": format!("start.gg error: {}", e)}), 502))
    }

    /// Resolve `dest` against a fresh `available_sets` read and assign it,
    /// returning a human label (`"station 3"` / `stream "socalrivals"`) for
    /// logging. Shared by `do_start_match` and `do_reassign_destination`.
    fn assign(
        &self,
        data: &Value,
        set_id: &Value,
        dest: &Destination,
    ) -> Result<String, (Value, u16)> {
        match dest {
            Destination::Station(num) => {
                let station_id = resolve_station_id(data, *num)?;
                if let Err(e) = self.startgg.assign_station(set_id, &station_id) {
                    return Err((
                        json!({"error": format!("start.gg station assignment failed: {}", e)}),
                        502,
                    ));
                }
                Ok(format!("station {num}"))
            }
            Destination::Stream(name) => {
                let stream_id = resolve_stream_id(data, name)?;
                if let Err(e) = self.startgg.assign_stream(set_id, &stream_id) {
                    return Err((
                        json!({"error": format!("start.gg stream assignment failed: {}", e)}),
                        502,
                    ));
                }
                Ok(format!("stream \"{name}\""))
            }
        }
    }

    /// Start a match on start.gg -- explicit operator action, same standing
    /// as `do_report`'s Report button (the mutation this calls,
    /// `markSetInProgress`, is "the TO's call" the rest of this app
    /// deliberately never invokes on its own). Optionally (re)assigns a
    /// station, a stream, or both first, as one user-facing action --
    /// start.gg lets a set sit at a physical station AND on a stream at the
    /// same time.
    ///
    /// The destination is never trusted blindly from the frontend: it is
    /// resolved to start.gg's opaque id server-side, against a fresh
    /// `available_sets` read, the same way `matching::map_slots_to_entrants`
    /// never trusts a client-supplied slot/entrant pairing without
    /// cross-checking.
    ///
    /// Per direct user instruction, this supersedes a prior session's more
    /// conservative decision here: a destination naming something DIFFERENT
    /// than what's already assigned used to be refused outright (409), on
    /// the reasoning that reassigning a set out from under whichever
    /// station it's bound to was a bigger, more surprising side effect than
    /// "start this set" asked for. The operator has since said explicitly
    /// that changing a set's station from inside this app is wanted, so
    /// that refusal is gone: whenever the requested destination differs
    /// from the current one (whether the set currently has none, or a
    /// different one), it's (re)assigned before `start_match` runs. If it
    /// already matches, assigning is simply skipped (nothing to do) and
    /// `start_match` still runs.
    ///
    /// If the destination requires an assignment and that assignment
    /// fails, `start_match` is never attempted -- a partial failure must
    /// never leave the operator thinking the match started when only the
    /// (failed) assignment was attempted, or believing an assignment
    /// happened when it didn't.
    pub fn do_start_match(
        &self,
        slug: &str,
        set_id: &Value,
        station_number: Option<i64>,
        stream_name: Option<String>,
    ) -> Result<Value, (Value, u16)> {
        if !self.startgg.enabled() {
            return Err((
                json!({"error": "No start.gg token configured on the hub."}),
                501,
            ));
        }
        let dests = Destination::from_parts(station_number, stream_name.clone());
        let data = self
            .startgg
            .available_sets(slug)
            .map_err(|e| (json!({"error": format!("start.gg error: {}", e)}), 502))?;
        let sets = data
            .get("sets")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let set_id_s = py_str(set_id);
        let target = sets
            .iter()
            .find(|s| py_str(s.get("id").unwrap_or(&Value::Null)) == set_id_s);
        let target = match target {
            Some(t) => t,
            None => {
                return Err((
                    json!({"error": "Set not found or not available to start."}),
                    404,
                ))
            }
        };
        reject_preview_set(set_id)?;

        // A set can carry both a station and a stream at once; assign each
        // requested one that differs from what's already set (an unchanged
        // destination is skipped -- nothing to do), then start.
        for d in &dests {
            if !d.matches_current(target) {
                let label = self.assign(&data, set_id, d)?;
                (self.log)(&format!("assigned set {set_id_s} to {label} on start.gg"));
            }
        }

        if let Err(e) = self.startgg.start_match(set_id) {
            return Err((
                json!({"error": format!("start.gg start match failed: {}", e)}),
                502,
            ));
        }
        (self.log)(&format!("started match for set {set_id_s} on start.gg"));
        Ok(json!({
            "ok": true, "setId": set_id,
            "stationAssigned": station_number, "streamAssigned": stream_name,
        }))
    }

    /// Change a set's station and/or stream on start.gg without starting it --
    /// for a set that's already playing (state 2, "playing now" in the
    /// Current Sets panel), where there's no "start" action to also fire
    /// alongside a destination change. Shares `do_start_match`'s security
    /// property: the destination is resolved to start.gg's opaque id
    /// server-side against a fresh `available_sets` read, never trusted as
    /// a raw id from the frontend. Only ever assigns -- never calls
    /// `start_match`.
    pub fn do_reassign_destination(
        &self,
        slug: &str,
        set_id: &Value,
        station_number: Option<i64>,
        stream_name: Option<String>,
    ) -> Result<Value, (Value, u16)> {
        if !self.startgg.enabled() {
            return Err((
                json!({"error": "No start.gg token configured on the hub."}),
                501,
            ));
        }
        let dests = Destination::from_parts(station_number, stream_name.clone());
        if dests.is_empty() {
            return Err((json!({"error": "No station or stream specified."}), 400));
        }
        let data = self
            .startgg
            .available_sets(slug)
            .map_err(|e| (json!({"error": format!("start.gg error: {}", e)}), 502))?;
        let sets = data
            .get("sets")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let set_id_s = py_str(set_id);
        let target = sets
            .iter()
            .find(|s| py_str(s.get("id").unwrap_or(&Value::Null)) == set_id_s);
        let target = match target {
            Some(t) => t,
            None => {
                return Err((
                    json!({"error": "Set not found or not available to start."}),
                    404,
                ))
            }
        };
        reject_preview_set(set_id)?;
        // Same both-at-once rule as do_start_match: assign each requested
        // destination that differs from the set's current one, skipping any
        // that already match (no redundant start.gg mutation).
        for d in &dests {
            if !d.matches_current(target) {
                let label = self.assign(&data, set_id, d)?;
                (self.log)(&format!("assigned set {set_id_s} to {label} on start.gg"));
            }
        }
        Ok(json!({
            "ok": true, "setId": set_id,
            "stationAssigned": station_number, "streamAssigned": stream_name,
        }))
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
            let now_swapped = !truthy(rec.get("swap"));
            rec["swap"] = json!(now_swapped);
            // The stored display mapping must flip WITH the flag — the report
            // path computes its mapping fresh (and honors swap), but the
            // console renders this stored view, and leaving it stale showed
            // the backwards pairing as if the swap never happened.
            {
                let summary = rec.get("set").cloned().unwrap_or_else(|| json!({}));
                rec["slotEntrants"] =
                    slot_entrants(&summary, rec.get("entrants"), &s.tag_map, now_swapped);
            }
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
        // "ok" and "startgg" are unchanged from before — the web console at
        // jugeeya.github.io/matchlogger/matchlogger.js and existing installs
        // read those two. "app" and "slug" are additive: a LAN discovery
        // scan (crate::discovery) uses "app" to confirm this is actually a
        // rivals-station-reporter hub (not just anything answering on the
        // port) and shows "slug" so an auto-connect is never a silent guess.
        // "serverTime" is also additive: the station's forwarder polls this
        // endpoint (crate::forwarder::Forwarder::clock_skew_check) to catch
        // a station system clock that has drifted from the hub's, since
        // replay matching and idle/pending timeouts all depend on wall
        // clock agreement between the two machines.
        "health" => (
            json!({
                "ok": true,
                "startgg": hub.startgg.enabled(),
                "app": APP_ID,
                "slug": hub.event_slug().unwrap_or_default(),
                "serverTime": now_sec(),
            }),
            200,
        ),
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

    /// A rebuild stops the old server and starts a new one back to back
    /// (same process, same port). Plain `TcpListener::bind` doesn't set
    /// `SO_REUSEADDR`, and on Windows in particular, closing a listening
    /// socket doesn't release the port instantly -- the very next bind
    /// attempt can lose to that delay and fail with "address already in
    /// use" (confirmed live: exactly this, os error 10048, on a real
    /// config-driven rebuild -- and confirmed that a short retry loop does
    /// NOT reliably cover it: the delay can run into multiple seconds, not
    /// milliseconds). Binding through `socket2` with `SO_REUSEADDR` set
    /// avoids the race at the OS level instead of racing it.
    fn bind(addr: &str) -> Result<std::net::TcpListener, String> {
        let sock_addr: std::net::SocketAddr = addr
            .parse()
            .map_err(|e| format!("bad bind address {addr}: {e}"))?;
        let domain = if sock_addr.is_ipv4() {
            socket2::Domain::IPV4
        } else {
            socket2::Domain::IPV6
        };
        let socket =
            socket2::Socket::new(domain, socket2::Type::STREAM, None).map_err(|e| e.to_string())?;
        socket.set_reuse_address(true).map_err(|e| e.to_string())?;
        socket.bind(&sock_addr.into()).map_err(|e| e.to_string())?;
        socket.listen(128).map_err(|e| e.to_string())?;
        Ok(socket.into())
    }

    pub fn start(&mut self) -> Result<String, String> {
        if self.srv.is_some() {
            return Ok(self.url());
        }
        let addr = format!("{}:{}", self.bind, self.port);
        // SO_REUSEADDR (see `bind` above) covers the OS-level TIME_WAIT
        // delay, but NOT a listener fd that is still momentarily alive:
        // tiny_http's own internal accept thread holds the previous socket,
        // and `Server`'s drop/unblock only SIGNALS that thread, never joins
        // it -- so a back-to-back stop -> start (a config-driven rebuild, or
        // the restart test) can reach this bind a few milliseconds before
        // the old fd is actually closed, which on Linux is EADDRINUSE no
        // matter what socket options the new bind sets. That window is
        // milliseconds, so a short bounded retry closes it; a port held by
        // some OTHER process still fails, just ~2s later.
        let listener = {
            let mut attempt = 0;
            loop {
                match Self::bind(&addr) {
                    Ok(l) => break l,
                    Err(_) if attempt < 20 => {
                        attempt += 1;
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        let server = tiny_http::Server::from_listener(listener, None).map_err(|e| e.to_string())?;
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

    /// A set still being played resolves its slot to entrant pairing.
    ///
    /// This is the case the console could not derive for itself: with no set
    /// winner there is no candidateWinnerEntrantId to anchor on, so before
    /// this every player in a live set displayed as unknown even though the
    /// hub already knew the pairing well enough to report it.
    #[test]
    fn slot_entrants_resolve_for_a_set_with_no_winner_yet() {
        let summary = json!({
            "complete": false, "winnerName": Value::Null,
            "players": [
                {"slot": 0, "name": "BRUJITA", "character": "Maypul", "wins": 2},
                {"slot": 1, "name": "JUGZ!", "character": "Clairen", "wins": 1},
            ],
        });
        let entrants = json!([{"id": "E3", "name": "Brujita"}, {"id": "E1", "name": "jugeeya"}]);
        let tag_map = matching::build_tag_map(Some(&json!({"JUGZ!": "jugeeya"})));

        let got = slot_entrants(&summary, Some(&entrants), &tag_map, false);
        assert_eq!(
            got,
            json!([
                {"slot": 0, "entrantId": "E3", "entrantName": "Brujita"},
                {"slot": 1, "entrantId": "E1", "entrantName": "jugeeya"},
            ]),
            "both slots pair, ordered by slot"
        );
    }

    /// No bracket set bound means nothing to pair against. Null rather than a
    /// fabricated pairing, so the console can say it does not know.
    #[test]
    fn slot_entrants_are_null_without_entrants() {
        let summary = json!({"players": [{"slot": 0, "name": "A"}, {"slot": 1, "name": "B"}]});
        let tag_map = HashMap::new();
        assert_eq!(slot_entrants(&summary, None, &tag_map, false), Value::Null);
        assert_eq!(
            slot_entrants(&summary, Some(&json!([])), &tag_map, false),
            Value::Null
        );
    }

    /// The operator's swap must flip the STORED display mapping, not just the
    /// fresh mapping the report path computes — a stale slotEntrants showed
    /// the backwards pairing in the console as if the swap never happened.
    #[test]
    fn do_swap_flips_the_stored_slot_entrants() {
        let fake = FakeStartgg::new();
        let mut h = Hub::new(None, None, Some(tags()), None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(SLUG, 1, &with(real_set(), &[("setId", json!("SWAPUI"))]))
            .unwrap();
        let before = h.get_set(SLUG, 1, &json!("SWAPUI")).unwrap();
        let before_first = before["slotEntrants"][0]["entrantName"].clone();
        assert!(!before_first.is_null(), "mapping resolved before the swap");

        h.do_swap(SLUG, 1, &json!("SWAPUI")).unwrap();
        let after = h.get_set(SLUG, 1, &json!("SWAPUI")).unwrap();
        assert_eq!(
            after["slotEntrants"][0]["entrantName"], before["slotEntrants"][1]["entrantName"],
            "slot 0 now shows the entrant slot 1 had  [{after}]"
        );
        assert_eq!(
            after["slotEntrants"][1]["entrantName"], before_first,
            "and vice versa"
        );
    }

    // ---- preferred_started_at: startedAt-over-startAt fallback ------------------
    // start.gg exposes two separate, similarly-named timestamps; which one
    // populates when a real match starts could not be verified live (see the
    // function's own doc and startgg.rs's STATION_SET_QUERY comment). These
    // confirm the documented, defensive fallback: prefer startedAt, fall back
    // to startAt only when startedAt is absent, and null when neither is.

    #[test]
    fn preferred_started_at_prefers_started_at_when_present() {
        let sg = json!({"startedAt": 1784879708i64, "startAt": 1784879700i64});
        assert_eq!(preferred_started_at(&sg), json!(1784879708i64));
    }

    #[test]
    fn preferred_started_at_falls_back_to_start_at_when_started_at_is_null() {
        let sg = json!({"startedAt": Value::Null, "startAt": 1784879700i64});
        assert_eq!(preferred_started_at(&sg), json!(1784879700i64));
    }

    #[test]
    fn preferred_started_at_falls_back_to_start_at_when_started_at_is_missing() {
        let sg = json!({"startAt": 1784879700i64});
        assert_eq!(preferred_started_at(&sg), json!(1784879700i64));
    }

    #[test]
    fn preferred_started_at_is_null_when_neither_field_is_present() {
        assert_eq!(preferred_started_at(&json!({})), Value::Null);
        assert_eq!(
            preferred_started_at(&json!({"startedAt": Value::Null, "startAt": Value::Null})),
            Value::Null
        );
        assert_eq!(preferred_started_at(&Value::Null), Value::Null);
    }

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
        /// Forces `set_games`'s answer for the "start.gg hasn't caught up
        /// yet" tests. `None` -- the default -- means "answer honestly",
        /// simulating a backend that already reflects the last push.
        set_games_override: Mutex<Option<Value>>,
        /// `set_state`'s answer, independent of `state` above: `state` is
        /// what `station_set`'s filtered lookup would find (and stays out of
        /// [1,2,6] once a set completes, per that query's own filter), while
        /// this is the direct-by-id state check that can still see a set
        /// after it drops out of that list. Defaults to "not completed";
        /// tests set it to 3 to simulate a TO finalizing the set on start.gg
        /// directly, bypassing this hub's Report button entirely.
        set_state_override: Mutex<Value>,
        /// Canned answer for `available_sets` -- the already-normalized
        /// `{sets, stations}` shape `do_start_match` consumes (parsing the
        /// raw GraphQL response is `startgg.rs`'s own concern, tested there;
        /// this fake sits at the trait boundary).
        available_sets_response: Mutex<Value>,
        /// Recorded (setId, stationId) pairs `assign_station` was called
        /// with -- lets tests assert the opaque id was resolved server-side,
        /// never the raw station number.
        assign_calls: Mutex<Vec<(Value, Value)>>,
        /// Same as `assign_calls`, for `assign_stream`.
        assign_stream_calls: Mutex<Vec<(Value, Value)>>,
        /// Recorded setIds `start_match` was called with.
        start_calls: Mutex<Vec<Value>>,
        assign_station_should_fail: Mutex<bool>,
        assign_stream_should_fail: Mutex<bool>,
        start_match_should_fail: Mutex<bool>,
        /// Extra fields merged onto `station_set`'s returned object -- used
        /// to simulate start.gg's `startedAt`/`startAt`/`totalGames`
        /// without disturbing every other test's `station_set` shape.
        /// Defaults to an empty object (nothing merged in).
        station_set_extra: Mutex<Value>,
        /// Per-station overrides for the setId `station_set` reports as
        /// "found" -- defaults to 105639152 for every station (as before this
        /// field existed) unless a test asks for a specific station to bind
        /// to a different fake start.gg set id. Needed so two different
        /// stations' records can be independently driven to two different
        /// `set_state` answers: the sweep tests need one externally-completed
        /// set and one that isn't, at the same time, which first requires two
        /// distinct set ids (the default single-id behavior gives every
        /// station the same matchedStartggSetId).
        station_set_ids: Mutex<HashMap<i64, Value>>,
        /// Per-set-id overrides for `set_state`, keyed by the set id's string
        /// form, consulted before the single `set_state_override` above. The
        /// single override is global to every set id, so it can't give two
        /// different sets two different remote states at once -- this can.
        set_state_overrides: Mutex<HashMap<String, Value>>,
    }

    impl FakeStartgg {
        fn new() -> Arc<FakeStartgg> {
            Arc::new(FakeStartgg {
                enabled: true,
                state: Mutex::new(2),
                live_pushes: Mutex::new(Vec::new()),
                reports: Mutex::new(Vec::new()),
                set_games_override: Mutex::new(None),
                set_state_override: Mutex::new(Value::Null),
                available_sets_response: Mutex::new(
                    json!({"sets": [], "stations": [], "streams": []}),
                ),
                assign_calls: Mutex::new(Vec::new()),
                assign_stream_calls: Mutex::new(Vec::new()),
                start_calls: Mutex::new(Vec::new()),
                assign_station_should_fail: Mutex::new(false),
                assign_stream_should_fail: Mutex::new(false),
                start_match_should_fail: Mutex::new(false),
                station_set_extra: Mutex::new(json!({})),
                station_set_ids: Mutex::new(HashMap::new()),
                set_state_overrides: Mutex::new(HashMap::new()),
            })
        }

        fn set_state(&self, st: i64) {
            *self.state.lock().unwrap() = st;
        }

        /// Merge extra fields (e.g. `startedAt`/`startAt`/`totalGames`) onto
        /// every subsequent `station_set` response.
        fn set_station_set_extra(&self, v: Value) {
            *self.station_set_extra.lock().unwrap() = v;
        }

        /// Bind a specific station to a specific (fake) start.gg set id,
        /// instead of the default 105639152 every station gets otherwise.
        fn set_station_set_id(&self, station: i64, set_id: Value) {
            self.station_set_ids.lock().unwrap().insert(station, set_id);
        }

        /// Answer `set_state` for one specific set id independently of the
        /// global `set_state_will_answer` override.
        fn set_state_will_answer_for(&self, set_id: &Value, v: Value) {
            self.set_state_overrides
                .lock()
                .unwrap()
                .insert(py_str(set_id), v);
        }

        fn pushes(&self) -> Vec<(Value, Value)> {
            self.live_pushes.lock().unwrap().clone()
        }

        fn reports(&self) -> Vec<(Value, Value, Option<Value>)> {
            self.reports.lock().unwrap().clone()
        }

        /// Set what `available_sets` answers -- already in the normalized
        /// `{sets, stations}` shape (see `available_sets_response`'s doc).
        fn set_available_sets(&self, v: Value) {
            *self.available_sets_response.lock().unwrap() = v;
        }

        fn assign_calls(&self) -> Vec<(Value, Value)> {
            self.assign_calls.lock().unwrap().clone()
        }

        fn assign_stream_calls(&self) -> Vec<(Value, Value)> {
            self.assign_stream_calls.lock().unwrap().clone()
        }

        fn start_calls(&self) -> Vec<Value> {
            self.start_calls.lock().unwrap().clone()
        }

        fn fail_assign_station(&self) {
            *self.assign_station_should_fail.lock().unwrap() = true;
        }

        fn fail_assign_stream(&self) {
            *self.assign_stream_should_fail.lock().unwrap() = true;
        }

        /// Simulate a set's remote state as seen by the direct-by-id check --
        /// independent of `set_state` above (which drives the FILTERED
        /// lookup `station_set` uses, and can't represent "completed" at
        /// all, since that state is exactly what falls out of that filter).
        fn set_state_will_answer(&self, v: Value) {
            *self.set_state_override.lock().unwrap() = v;
        }

        /// Simulate start.gg not (yet) reflecting the last push -- or
        /// reflecting something else entirely.
        fn set_games_will_answer(&self, v: Option<Value>) {
            *self.set_games_override.lock().unwrap() = v;
        }

        /// Turns pushed gameData (`{gameNum, winnerId, selections:
        /// [{entrantId, characterId}]}`, ids as strings) into the shape
        /// `set_games` reads back from start.gg (`{orderNum, winnerId,
        /// selections:[{entrant:{id}, character:{id}}]}`, ids as JSON
        /// numbers). Returning numbers here, where the push used strings, is
        /// deliberate: it's what the real API actually does (confirmed by
        /// introspecting the live schema), and it's exactly the mismatch
        /// `live_push_confirmed`'s id normalization has to see past.
        fn honest_read_back(pushed: &Value) -> Value {
            let empty: Vec<Value> = Vec::new();
            let games: Vec<Value> = pushed
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|g| {
                    let as_num = |v: &Value| -> Value {
                        v.as_str()
                            .and_then(|s| s.parse::<i64>().ok())
                            .map(|n| json!(n))
                            .unwrap_or_else(|| v.clone())
                    };
                    let selections: Vec<Value> = g
                        .get("selections")
                        .and_then(|v| v.as_array())
                        .unwrap_or(&empty)
                        .iter()
                        .map(|s| {
                            json!({
                                "entrant": {"id": as_num(s.get("entrantId").unwrap_or(&Value::Null))},
                                "character": {"id": s.get("characterId").cloned().unwrap_or(Value::Null)},
                            })
                        })
                        .collect();
                    json!({
                        "orderNum": g.get("gameNum").cloned().unwrap_or(Value::Null),
                        "winnerId": g.get("winnerId").map(as_num).unwrap_or(Value::Null),
                        "selections": selections,
                    })
                })
                .collect();
            json!(games)
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
            station: i64,
            _max_age: f64,
        ) -> Result<Value, StartggError> {
            let set_id = self
                .0
                .station_set_ids
                .lock()
                .unwrap()
                .get(&station)
                .cloned()
                .unwrap_or(json!(105639152));
            let mut result = json!({
                "found": true, "setId": set_id, "state": *self.0.state.lock().unwrap(),
                "fullRoundText": "Winners Round 1", "entrants": entrants(),
            });
            if let Some(extra) = self.0.station_set_extra.lock().unwrap().as_object() {
                for (k, v) in extra {
                    result[k] = v.clone();
                }
            }
            Ok(result)
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
        fn set_games(&self, _set_id: &Value) -> Result<Value, StartggError> {
            if let Some(v) = self.0.set_games_override.lock().unwrap().clone() {
                return Ok(v);
            }
            let pushes = self.0.live_pushes.lock().unwrap();
            Ok(pushes
                .last()
                .map(|(_, gd)| FakeStartgg::honest_read_back(gd))
                .unwrap_or(Value::Null))
        }
        fn set_state(&self, set_id: &Value) -> Result<Value, StartggError> {
            let key = py_str(set_id);
            if let Some(v) = self.0.set_state_overrides.lock().unwrap().get(&key) {
                return Ok(v.clone());
            }
            Ok(self.0.set_state_override.lock().unwrap().clone())
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
        fn available_sets(&self, _slug: &str) -> Result<Value, StartggError> {
            Ok(self.0.available_sets_response.lock().unwrap().clone())
        }
        fn assign_station(&self, set_id: &Value, station_id: &Value) -> Result<(), StartggError> {
            if *self.0.assign_station_should_fail.lock().unwrap() {
                return Err(StartggError("assignStation failed".to_string()));
            }
            self.0
                .assign_calls
                .lock()
                .unwrap()
                .push((set_id.clone(), station_id.clone()));
            Ok(())
        }
        fn assign_stream(&self, set_id: &Value, stream_id: &Value) -> Result<(), StartggError> {
            if *self.0.assign_stream_should_fail.lock().unwrap() {
                return Err(StartggError("assignStream failed".to_string()));
            }
            self.0
                .assign_stream_calls
                .lock()
                .unwrap()
                .push((set_id.clone(), stream_id.clone()));
            Ok(())
        }
        fn start_match(&self, set_id: &Value) -> Result<(), StartggError> {
            if *self.0.start_match_should_fail.lock().unwrap() {
                return Err(StartggError("markSetInProgress failed".to_string()));
            }
            self.0.start_calls.lock().unwrap().push(set_id.clone());
            Ok(())
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
            // Keyed by station number, not by slug: the snapshot is already
            // scoped to one event, and the console indexes stations by number.
            !snap["stations"]["1"].is_null(),
            "station heartbeat reached the hub"
        );
        assert_eq!(
            snap["stations"]["1"]["startgg"]["setId"],
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

        // ---- the console is scoped to the event this hub is running ----------
        // hub-state.json accumulates every event an install has ever run and is
        // never pruned, so an unscoped console showed last week's bracket mixed
        // into this week's.
        h.handle_ingest("some-other-event", 9, &real_set()).unwrap();
        let all = h.snapshot()["sets"].as_array().unwrap().len();
        h.set_event_slug(SLUG);
        let scoped = h.snapshot();
        assert_eq!(
            scoped["sets"].as_array().unwrap().len(),
            all - 1,
            "the other event's set is not in this event's console"
        );
        assert!(
            scoped["stations"]["9"].is_null() && !scoped["stations"]["1"].is_null(),
            "stations are scoped to this event too, and keyed by number"
        );
        h.set_event_slug("");
        assert_eq!(
            h.snapshot()["sets"].as_array().unwrap().len(),
            all,
            "a hub with no event configured has nothing to scope to, so shows everything"
        );
        h.set_event_slug(SLUG);

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

    /// Regression test for the live-reproduced bug: a config-driven rebuild
    /// stops the old server and starts a new one on the same port back to
    /// back, with nothing pacing the two calls apart. A plain
    /// `TcpListener::bind` lost this race often enough in practice (WSAEADDRINUSE,
    /// os error 10048) that even a 5-attempt/200ms retry loop didn't reliably
    /// cover it. `HubServer::bind`'s `SO_REUSEADDR` should make the very next
    /// bind on the same port succeed immediately, no retry needed.
    #[test]
    fn restart_on_the_same_port_back_to_back_does_not_lose_the_bind_race() {
        let h = Arc::new(Hub::new(
            Some(KEY),
            None,
            Some(tags()),
            None,
            None,
            None,
            None,
            None,
        ));

        // free_port() drops its probe listener before returning, so a test
        // running in parallel can steal the port in that gap -- retry with a
        // fresh candidate instead of flaking. The property under test is the
        // RESTART bind below, not this first one.
        let (mut server, port) = (0..10)
            .find_map(|_| {
                let port = free_port();
                let mut s = HubServer::new(h.clone(), port, "127.0.0.1");
                s.start().ok().map(|_| (s, port))
            })
            .expect("one of ten fresh ports binds");
        server.stop();

        let mut server = HubServer::new(h, port, "127.0.0.1");
        server
            .start()
            .expect("restarting on the same port immediately after stop must not race the bind");
        server.stop();
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

    // ---- start.gg's authoritative startedAt/totalGames thread onto the record ---
    // record_for (via bind_station_set) is where sg's fields land on a hub
    // record; this confirms startggStartedAt/startggTotalGames actually reach
    // the stored record, not just that the pure preferred_started_at helper
    // works in isolation.
    #[test]
    fn record_carries_startgg_started_at_and_total_games_from_the_station_binding() {
        let workdir = tmpdir("hubtest_startgg_started_at");
        let fake = FakeStartgg::new();
        fake.set_station_set_extra(json!({
            "startedAt": 1784879708i64, "startAt": 1784879700i64, "totalGames": 5,
        }));
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(SLUG, 1, &with(real_set(), &[("setId", json!("SGSTART"))]))
            .unwrap();
        let rec = h.get_set(SLUG, 1, &json!("SGSTART")).unwrap();
        assert_eq!(
            rec["startggStartedAt"],
            json!(1784879708i64),
            "startedAt preferred over startAt on the stored record  [{rec}]"
        );
        assert_eq!(
            rec["startggTotalGames"],
            json!(5),
            "totalGames threaded onto the stored record  [{rec}]"
        );

        // Now simulate a station binding that only ever exposed startAt
        // (startedAt absent) -- rebind should carry the fallback through too.
        fake.set_station_set_extra(json!({"startAt": 1784879700i64}));
        let rebound = h
            .rebind(SLUG, 1, &json!("SGSTART"))
            .expect("rebind finds the existing record");
        assert_eq!(
            rebound["startggStartedAt"],
            json!(1784879700i64),
            "rebind falls back to startAt when startedAt is absent  [{rebound}]"
        );

        let _ = fs::remove_dir_all(&workdir);
    }

    // ---- set_open pre-binding -------------------------------------------------
    // This app's own set_machine only ever writes "set_open"/"idle" (never the
    // legacy Python sender's "set_start"), so the pre-bind has to trigger on
    // the first set_open heartbeat of a NEW setId -- and re-trigger when the
    // setId changes, or a binding cached at report time (rebind) would keep
    // pointing the NEXT set's games at the PREVIOUS bracket set.

    #[test]
    fn a_set_open_heartbeat_for_a_new_set_binds_the_station() {
        let fake = FakeStartgg::new();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .handle_current(
                SLUG,
                1,
                Some(&json!({"state": "set_open", "setId": "L1", "matchCount": 1})),
            )
            .unwrap();
        assert_eq!(
            res["startgg"]["setId"],
            json!(105639152),
            "the first set_open heartbeat pre-binds the station's start.gg set  [{res}]"
        );
    }

    #[test]
    fn a_repeat_set_open_heartbeat_keeps_the_original_binding() {
        let fake = FakeStartgg::new();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(
            SLUG,
            1,
            Some(&json!({"state": "set_open", "setId": "L1", "matchCount": 1})),
        )
        .unwrap();
        // start.gg's answer for this station changes mid-set (TO shuffling
        // assignments): heartbeats for the SAME setId must not chase it.
        fake.set_station_set_id(1, json!("SOMETHING-ELSE"));
        let res = h
            .handle_current(
                SLUG,
                1,
                Some(&json!({"state": "set_open", "setId": "L1", "matchCount": 2})),
            )
            .unwrap();
        assert_eq!(
            res["startgg"]["setId"],
            json!(105639152),
            "a mid-set heartbeat preserves the set's original binding  [{res}]"
        );
    }

    #[test]
    fn a_new_set_id_refreshes_a_stale_binding() {
        // The bracket-corruption scenario this guards against: set A's
        // binding is cached on the station record (handle_current or a
        // report-time rebind); set B then starts at the same station. Its
        // set_open heartbeat carries a new setId, so the binding must be
        // looked up fresh -- NOT carried over from set A, which would
        // live-push set B's games onto set A's bracket entry.
        let fake = FakeStartgg::new();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(
            SLUG,
            1,
            Some(&json!({"state": "set_open", "setId": "A", "matchCount": 1})),
        )
        .unwrap();
        fake.set_station_set_id(1, json!("SG-B"));
        let res = h
            .handle_current(
                SLUG,
                1,
                Some(&json!({"state": "set_open", "setId": "B", "matchCount": 1})),
            )
            .unwrap();
        assert_eq!(
            res["startgg"]["setId"],
            json!("SG-B"),
            "a new setId at the station re-binds against a fresh lookup  [{res}]"
        );
    }

    #[test]
    fn an_idle_heartbeat_preserves_the_binding() {
        let fake = FakeStartgg::new();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(
            SLUG,
            1,
            Some(&json!({"state": "set_open", "setId": "L1", "matchCount": 1})),
        )
        .unwrap();
        let res = h
            .handle_current(SLUG, 1, Some(&json!({"state": "idle"})))
            .unwrap();
        assert_eq!(
            res["startgg"]["setId"],
            json!(105639152),
            "going idle between games/sets keeps the binding for the eventual ingest  [{res}]"
        );
    }

    // ---- Report refuses a set someone already finalized on start.gg directly ---
    // `reportable` only ever gets set false by this hub's own bind/rebind logic,
    // so a set we already matched stays "reportable" as far as OUR state is
    // concerned no matter what happened to it out from under us. do_report has
    // to check the live state itself, right before the write.

    #[test]
    fn report_is_refused_when_start_gg_already_shows_it_completed() {
        let workdir = tmpdir("hubtest_alreadydone");
        let fake = FakeStartgg::new();
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(SLUG, 1, &with(real_set(), &[("setId", json!("AR1"))]))
            .unwrap();
        let before = h.get_set(SLUG, 1, &json!("AR1")).unwrap();
        assert_eq!(before["reportable"], json!(true), "normally reportable");

        // A TO finalized this exact set on start.gg's own page -- nothing
        // about that touches this hub, so `reportable` above never moved.
        fake.set_state_will_answer(json!(matching::STARTGG_STATE_COMPLETED));

        let err = h
            .do_report(SLUG, 1, &json!("AR1"), &json!(24186345))
            .expect_err("must not attempt to write over an already-completed set");
        assert_eq!(err.1, 409);
        assert!(
            err.0["error"]
                .as_str()
                .unwrap_or("")
                .contains("already reported"),
            "error explains why  [{}]",
            err.0
        );
        assert_eq!(
            fake.reports().len(),
            0,
            "no write was attempted against start.gg"
        );

        let after = h.get_set(SLUG, 1, &json!("AR1")).unwrap();
        assert_eq!(
            after["reportable"],
            json!(false),
            "the record reflects it's settled, not still actionable"
        );
        assert_eq!(after["status"], json!("already reported on start.gg"));

        let _ = fs::remove_dir_all(&workdir);
    }

    #[test]
    fn report_still_succeeds_when_start_gg_does_not_show_it_completed() {
        let workdir = tmpdir("hubtest_notdone");
        let fake = FakeStartgg::new();
        // Explicit, not just the default: this confirms the check itself
        // (state != 3) rather than merely "the check never ran".
        fake.set_state_will_answer(json!(matching::STARTGG_STATE_ONGOING));
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(SLUG, 1, &with(real_set(), &[("setId", json!("AR2"))]))
            .unwrap();

        let res = h
            .do_report(SLUG, 1, &json!("AR2"), &json!(24186345))
            .expect("a set start.gg doesn't show as completed reports normally");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(fake.reports().len(), 1, "the report reached start.gg");

        let _ = fs::remove_dir_all(&workdir);
    }

    // ---- sweep_reported_elsewhere: the periodic safety net -----------------------
    // do_report's own check (settle_if_reported_elsewhere) only runs at the
    // moment someone clicks Report; this proves the same settling logic also
    // runs proactively over the whole awaiting-report list, touching only
    // records that are actually awaiting report.

    #[test]
    fn sweep_settles_only_the_externally_completed_awaiting_report_record_and_ignores_live() {
        let workdir = tmpdir("hubtest_sweep");
        let fake = FakeStartgg::new();
        // Distinct fake start.gg set ids per station -- the default (every
        // station sharing one id) would make it impossible to drive two
        // different stations' sets to two different remote states.
        fake.set_station_set_id(1, json!(910001));
        fake.set_station_set_id(2, json!(910002));
        fake.set_station_set_id(3, json!(910003));
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        // Two awaiting-report ("matched") records at two different stations.
        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(
            SLUG,
            1,
            &with(real_set(), &[("setId", json!("SWEEP-DONE"))]),
        )
        .unwrap();
        h.handle_current(SLUG, 2, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(
            SLUG,
            2,
            &with(real_set(), &[("setId", json!("SWEEP-STILL-GOING"))]),
        )
        .unwrap();
        assert_eq!(
            h.get_set(SLUG, 1, &json!("SWEEP-DONE")).unwrap()["status"],
            json!("matched"),
            "fixture assumption: awaiting report before the sweep"
        );
        assert_eq!(
            h.get_set(SLUG, 2, &json!("SWEEP-STILL-GOING")).unwrap()["status"],
            json!("matched"),
            "fixture assumption: awaiting report before the sweep"
        );

        // A third record, still "live" (in progress, not awaiting report) --
        // must never be touched, even though its remote state also reads
        // back as completed. Proves the sweep skips it because of its local
        // status, not because the remote check merely never runs for it.
        h.handle_current(SLUG, 3, Some(&json!({"state": "set_start"})))
            .unwrap();
        let live_set = with(
            real_set(),
            &[("setId", json!("SWEEP-LIVE")), ("complete", json!(false))],
        );
        h.handle_live(SLUG, 3, &live_set).unwrap();
        assert_eq!(
            h.get_set(SLUG, 3, &json!("SWEEP-LIVE")).unwrap()["status"],
            json!("live"),
            "fixture assumption: still live before the sweep"
        );

        fake.set_state_will_answer_for(&json!(910001), json!(matching::STARTGG_STATE_COMPLETED));
        fake.set_state_will_answer_for(&json!(910002), json!(matching::STARTGG_STATE_ONGOING));
        fake.set_state_will_answer_for(&json!(910003), json!(matching::STARTGG_STATE_COMPLETED));

        h.sweep_reported_elsewhere(SLUG);

        let done = h.get_set(SLUG, 1, &json!("SWEEP-DONE")).unwrap();
        assert_eq!(
            done["status"],
            json!("already reported on start.gg"),
            "settled by the sweep  [{done}]"
        );
        assert_eq!(done["reportable"], json!(false));

        let still_going = h.get_set(SLUG, 2, &json!("SWEEP-STILL-GOING")).unwrap();
        assert_eq!(
            still_going["status"],
            json!("matched"),
            "untouched -- start.gg does not show it completed  [{still_going}]"
        );

        let live = h.get_set(SLUG, 3, &json!("SWEEP-LIVE")).unwrap();
        assert_eq!(
            live["status"],
            json!("live"),
            "a live-status record is never touched by the sweep, even though its \
             remote state reads back as completed  [{live}]"
        );

        let _ = fs::remove_dir_all(&workdir);
    }

    #[test]
    fn sweep_is_a_quiet_noop_when_nothing_needs_settling() {
        let workdir = tmpdir("hubtest_sweep_quiet");
        let fake = FakeStartgg::new();
        fake.set_state_will_answer(json!(matching::STARTGG_STATE_ONGOING));
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        h.handle_ingest(SLUG, 1, &with(real_set(), &[("setId", json!("SWEEP-OK"))]))
            .unwrap();
        let before = h.get_set(SLUG, 1, &json!("SWEEP-OK")).unwrap();

        h.sweep_reported_elsewhere(SLUG);

        let after = h.get_set(SLUG, 1, &json!("SWEEP-OK")).unwrap();
        assert_eq!(
            after, before,
            "nothing changed when start.gg shows nothing completed"
        );

        let _ = fs::remove_dir_all(&workdir);
    }

    // ---- live pushes are confirmed by reading the set back, not assumed ---------
    // updateBracketSet's response only echoes {id state}; it never says whether
    // the game data itself landed. These exercise the read-back this hub does
    // instead of trusting a bare "the HTTP call didn't error".

    #[test]
    fn live_push_is_confirmed_once_start_gg_reflects_it() {
        let workdir = tmpdir("hubtest_liveconfirm_ok");
        let fake = FakeStartgg::new();
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        let live_set = with(
            real_set(),
            &[("setId", json!("LC1")), ("complete", json!(false))],
        );
        let res = h.handle_live(SLUG, 1, &live_set).unwrap();

        assert_eq!(res["live"], json!(true));
        // The fake's default set_games answer is built from the same push it
        // just recorded (with ids re-typed to numbers, as start.gg actually
        // returns them), so a healthy backend confirms on the very next read.
        assert_eq!(
            res["confirmed"],
            json!(true),
            "a backend that already reflects the push must confirm  [{res}]"
        );

        // And the confirmation is on the STORED record, not just this call's
        // response -- that's what the operator console actually reads.
        let rec = h.get_set(SLUG, 1, &json!("LC1")).unwrap();
        assert_eq!(
            rec["liveConfirmed"],
            json!(true),
            "confirmation persists on the record  [{rec}]"
        );

        let _ = fs::remove_dir_all(&workdir);
    }

    #[test]
    fn live_push_stays_unconfirmed_while_start_gg_has_not_caught_up() {
        let workdir = tmpdir("hubtest_liveconfirm_lag");
        let fake = FakeStartgg::new();
        // Simulates read-after-write lag: the push succeeds, but a read right
        // after still sees nothing for the set.
        fake.set_games_will_answer(Some(Value::Null));
        let mut h = Hub::new(
            None,
            None,
            Some(tags()),
            None,
            Some(path_str(&workdir.join("h.json"))),
            None,
            None,
            Some(path_str(&workdir.join("learned.json"))),
        );
        h.startgg = Box::new(Shared(fake.clone()));

        h.handle_current(SLUG, 1, Some(&json!({"state": "set_start"})))
            .unwrap();
        let live_set = with(
            real_set(),
            &[("setId", json!("LC2")), ("complete", json!(false))],
        );
        let res = h.handle_live(SLUG, 1, &live_set).unwrap();

        // The push itself still succeeded -- lag isn't a push failure.
        assert_eq!(res["live"], json!(true));
        assert_eq!(
            res["confirmed"],
            json!(false),
            "not confirmed yet is not the same as failed  [{res}]"
        );
        let rec = h.get_set(SLUG, 1, &json!("LC2")).unwrap();
        assert_eq!(rec["liveConfirmed"], json!(false));

        // The next tick (the station keeps posting live updates on its own
        // poll interval) is what settles it, once start.gg has caught up --
        // this is the "poll" -- across natural ticks, not a busy-wait here.
        fake.set_games_will_answer(None);
        let res2 = h.handle_live(SLUG, 1, &live_set).unwrap();
        assert_eq!(res2["confirmed"], json!(true), "the next tick confirms it");

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

    // ---- Start Match: available_sets query wrapper -----------------------------

    #[test]
    fn available_sets_wrapper_delegates_to_startgg() {
        let fake = FakeStartgg::new();
        let payload = json!({
            "sets": [{"id": "S1", "fullRoundText": "WQF", "station": Value::Null,
                       "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}]}],
            "stations": [{"id": "opaque-1", "number": 1}],
        });
        fake.set_available_sets(payload.clone());
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake));
        assert_eq!(
            h.available_sets(SLUG).unwrap(),
            payload,
            "the wrapper hands back exactly what the start.gg client returned"
        );
    }

    #[test]
    fn available_sets_wrapper_errors_without_a_token() {
        // No override: the real (disabled, no token) Startgg client.
        let h = Hub::new(None, None, None, None, None, None, None, None);
        let err = h.available_sets(SLUG).unwrap_err();
        assert_eq!(
            err.1, 501,
            "no token configured -> 501, not a silent empty list"
        );
    }

    // ---- Start Match: do_start_match --------------------------------------------
    // markSetInProgress is "the TO's call" -- these confirm it's reached only
    // through this explicit path, station assignment is resolved server-side
    // (never trusting a raw number from the frontend), and a failed
    // assignment never lets start_match run anyway.

    fn set_with_station(id: &str, station: Value) -> Value {
        json!({
            "id": id, "fullRoundText": "Winners Quarter-Final", "station": station,
            "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}],
        })
    }

    fn set_with_stream(id: &str, stream: Value) -> Value {
        json!({
            "id": id, "fullRoundText": "Winners Quarter-Final", "stream": stream,
            "entrants": [{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}],
        })
    }

    fn stations_list() -> Value {
        json!([{"id": "opaque-st-1", "number": 1}, {"id": "opaque-st-2", "number": 2}])
    }

    fn streams_list() -> Value {
        json!([{"id": "opaque-stream-1", "name": "socalrivals"}])
    }

    #[test]
    fn do_start_match_without_a_station_only_calls_start_match() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), None, None)
            .expect("start match with no station succeeds");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.start_calls(),
            vec![json!("S1")],
            "start_match called with the set id"
        );
        assert!(
            fake.assign_calls().is_empty(),
            "no station requested -> assign_station is never called"
        );
    }

    #[test]
    fn do_start_match_with_a_station_and_none_assigned_assigns_then_starts() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), Some(2), None)
            .expect("assign then start succeeds");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.assign_calls(),
            vec![(json!("S1"), json!("opaque-st-2"))],
            "station number 2 resolved to its opaque id, not passed as the raw number"
        );
        assert_eq!(
            fake.start_calls(),
            vec![json!("S1")],
            "start_match runs after a successful assignment"
        );
    }

    #[test]
    fn do_start_match_with_the_same_station_already_assigned_skips_assign_and_starts() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", json!(2))],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), Some(2), None)
            .expect("requesting the station it already has succeeds");
        assert_eq!(res["ok"], json!(true));
        assert!(
            fake.assign_calls().is_empty(),
            "already assigned to the requested station -> no redundant assign_station call"
        );
        assert_eq!(fake.start_calls(), vec![json!("S1")]);
    }

    #[test]
    fn do_start_match_with_a_different_station_already_assigned_reassigns_then_starts() {
        // Supersedes the prior session's refuse-409 decision, per direct
        // user instruction: requesting a different station than the one
        // already assigned now (re)assigns it -- resolved to the new
        // station's opaque id, never the raw number -- and then starts,
        // instead of refusing outright.
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", json!(1))],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), Some(2), None)
            .expect("a set already on a different station can be moved and started");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.assign_calls(),
            vec![(json!("S1"), json!("opaque-st-2"))],
            "reassigned to station 2's resolved opaque id, not the raw number"
        );
        assert_eq!(
            fake.start_calls(),
            vec![json!("S1")],
            "start_match runs after the reassignment succeeds"
        );
    }

    #[test]
    fn do_start_match_a_failed_assignment_never_calls_start_match() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        fake.fail_assign_station();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_start_match(SLUG, &json!("S1"), Some(2), None)
            .expect_err("a failed assignment must surface as an error");
        assert_eq!(err.1, 502);
        assert!(
            fake.start_calls().is_empty(),
            "a failed assign_station must not let start_match run -- the operator must never \
             be shown a partial success as a clean one"
        );
    }

    #[test]
    fn do_start_match_unknown_set_id_is_refused() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_start_match(SLUG, &json!("NOPE"), None, None)
            .expect_err("a set not in the available list must not be started blindly");
        assert_eq!(err.1, 404);
        assert!(fake.start_calls().is_empty());
    }

    /// A bracket not yet started on start.gg reports placeholder set ids
    /// ("preview_3396320_1_0"). Mutating one always fails upstream, and
    /// start.gg says only "An unknown error has occurred" -- so refuse it here
    /// with something true, and don't spend the round trip.
    #[test]
    fn do_start_match_refuses_a_preview_set_from_an_unstarted_bracket() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("preview_3396320_1_0", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_start_match(SLUG, &json!("preview_3396320_1_0"), Some(2), None)
            .expect_err("a preview set cannot be started");
        assert_eq!(err.1, 409);
        assert!(
            err.0["error"]
                .as_str()
                .unwrap_or("")
                .contains("hasn't been started"),
            "the message must name the real cause, got: {}",
            err.0["error"]
        );
        assert!(
            fake.assign_calls().is_empty() && fake.start_calls().is_empty(),
            "nothing may be sent to start.gg for a set that cannot accept it"
        );
    }

    #[test]
    fn do_reassign_destination_refuses_a_preview_set() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("preview_3396320_1_1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_reassign_destination(SLUG, &json!("preview_3396320_1_1"), Some(2), None)
            .expect_err("a preview set cannot be reassigned");
        assert_eq!(err.1, 409);
        assert!(fake.assign_calls().is_empty());
    }

    /// Only the observed `preview` prefix counts. Real ids -- numeric in
    /// practice, but the guard must not assume that -- all pass through.
    /// Anything stricter would risk blocking a live bracket, which is worse
    /// than the failure being guarded against.
    #[test]
    fn only_preview_prefixed_set_ids_are_treated_as_preview() {
        use crate::startgg::is_preview_set_id;
        assert!(is_preview_set_id(&json!("preview_3396320_1_0")));
        assert!(!is_preview_set_id(&json!(105639152)));
        assert!(!is_preview_set_id(&json!("105639152")));
        assert!(!is_preview_set_id(&json!("S1")));
        assert!(!is_preview_set_id(&Value::Null));
    }

    #[test]
    fn do_start_match_requires_a_start_gg_token() {
        let h = Hub::new(None, None, None, None, None, None, None, None);
        let err = h
            .do_start_match(SLUG, &json!("S1"), None, None)
            .unwrap_err();
        assert_eq!(err.1, 501);
    }

    #[test]
    fn do_start_match_with_a_stream_assigns_then_starts() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_stream("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), None, Some("socalrivals".to_string()))
            .expect("assign a stream then start succeeds");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.assign_stream_calls(),
            vec![(json!("S1"), json!("opaque-stream-1"))],
            "stream name resolved to its opaque id, not passed as the raw name"
        );
        assert_eq!(fake.start_calls(), vec![json!("S1")]);
        assert!(
            fake.assign_calls().is_empty(),
            "a stream destination must never call assign_station"
        );
    }

    #[test]
    fn do_start_match_with_the_same_stream_already_assigned_skips_assign_and_starts() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_stream("S1", json!("socalrivals"))],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), None, Some("socalrivals".to_string()))
            .expect("requesting the stream it already has succeeds");
        assert_eq!(res["ok"], json!(true));
        assert!(
            fake.assign_stream_calls().is_empty(),
            "already assigned to the requested stream -> no redundant assign_stream call"
        );
        assert_eq!(fake.start_calls(), vec![json!("S1")]);
    }

    #[test]
    fn do_start_match_with_an_unknown_stream_name_is_refused() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_stream("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_start_match(SLUG, &json!("S1"), None, Some("nonexistent".to_string()))
            .expect_err("a stream name not on this tournament must be refused");
        assert_eq!(err.1, 404);
        assert!(fake.assign_stream_calls().is_empty());
        assert!(fake.start_calls().is_empty());
    }

    #[test]
    fn do_start_match_with_both_a_station_and_a_stream_assigns_both_then_starts() {
        // start.gg lets a set sit at a physical station AND on a stream at
        // once (e.g. Station 1 + "socalrivals"), so requesting both assigns
        // both -- each resolved to its opaque id -- before starting.
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), Some(2), Some("socalrivals".to_string()))
            .expect("a station and a stream at once assigns both, then starts");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.assign_calls(),
            vec![(json!("S1"), json!("opaque-st-2"))]
        );
        assert_eq!(
            fake.assign_stream_calls(),
            vec![(json!("S1"), json!("opaque-stream-1"))]
        );
        assert_eq!(fake.start_calls(), vec![json!("S1")]);
    }

    #[test]
    fn do_start_match_with_both_only_assigns_the_one_that_changed() {
        // Already at station 2, stream requested on top: the matching
        // station is skipped (no redundant mutation), only the stream is
        // assigned, and the match still starts.
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", json!(2))],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_start_match(SLUG, &json!("S1"), Some(2), Some("socalrivals".to_string()))
            .expect("an unchanged station alongside a new stream succeeds");
        assert_eq!(res["ok"], json!(true));
        assert!(
            fake.assign_calls().is_empty(),
            "already on station 2 -> no redundant assign_station call"
        );
        assert_eq!(
            fake.assign_stream_calls(),
            vec![(json!("S1"), json!("opaque-stream-1"))]
        );
        assert_eq!(fake.start_calls(), vec![json!("S1")]);
    }

    #[test]
    fn do_start_match_a_failed_stream_assignment_never_calls_start_match() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_stream("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        fake.fail_assign_stream();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_start_match(SLUG, &json!("S1"), None, Some("socalrivals".to_string()))
            .expect_err("a failed stream assignment must surface as an error");
        assert_eq!(err.1, 502);
        assert!(
            fake.start_calls().is_empty(),
            "a failed assign_stream must not let start_match run"
        );
    }

    // ---- Current Sets: do_reassign_destination -----------------------------
    // Changes a set's station or stream without starting it -- for a set
    // that's already playing, where there's no "start" action to also fire.

    #[test]
    fn do_reassign_station_assigns_when_none_set() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(2), None)
            .expect("assigning a station to a set with none succeeds");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(res["stationAssigned"], json!(2));
        assert_eq!(
            fake.assign_calls(),
            vec![(json!("S1"), json!("opaque-st-2"))],
            "station number 2 resolved to its opaque id, not passed as the raw number"
        );
        assert!(
            fake.start_calls().is_empty(),
            "do_reassign_station never calls start_match"
        );
    }

    #[test]
    fn do_reassign_station_reassigns_when_a_different_one_is_set() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", json!(1))],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(2), None)
            .expect("moving a set already on station 1 to station 2 succeeds");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.assign_calls(),
            vec![(json!("S1"), json!("opaque-st-2"))],
            "resolved to the NEW station's opaque id, not the old one"
        );
        assert!(fake.start_calls().is_empty());
    }

    #[test]
    fn do_reassign_station_resolves_the_station_number_securely() {
        // A raw/opaque-looking id passed as the "number" must not be trusted
        // directly -- it has to match an actual station's plain `number`
        // from a fresh available_sets read, same as do_start_match.
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(99), None)
            .expect_err("station 99 doesn't exist on this event");
        assert_eq!(err.1, 404);
        assert!(
            fake.assign_calls().is_empty(),
            "an unresolvable station number must never reach assign_station"
        );
    }

    #[test]
    fn do_reassign_station_unknown_set_id_is_refused() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_reassign_destination(SLUG, &json!("NOPE"), Some(2), None)
            .expect_err("a set not in the available list must not be reassigned blindly");
        assert_eq!(err.1, 404);
        assert!(fake.assign_calls().is_empty());
    }

    #[test]
    fn do_reassign_station_a_failed_assignment_surfaces_as_an_error() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
        }));
        fake.fail_assign_station();
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(2), None)
            .expect_err("a failed assignment must surface as an error");
        assert_eq!(err.1, 502);
    }

    #[test]
    fn do_reassign_station_requires_a_start_gg_token() {
        let h = Hub::new(None, None, None, None, None, None, None, None);
        let err = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(2), None)
            .unwrap_err();
        assert_eq!(err.1, 501);
    }

    #[test]
    fn do_reassign_destination_assigns_a_stream() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_stream("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_reassign_destination(SLUG, &json!("S1"), None, Some("socalrivals".to_string()))
            .expect("assigning a stream to a set with none succeeds");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(res["streamAssigned"], json!("socalrivals"));
        assert_eq!(
            fake.assign_stream_calls(),
            vec![(json!("S1"), json!("opaque-stream-1"))],
            "stream name resolved to its opaque id, not passed as the raw name"
        );
        assert!(
            fake.start_calls().is_empty(),
            "do_reassign_destination never calls start_match"
        );
    }

    #[test]
    fn do_reassign_destination_requires_a_destination() {
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let err = h
            .do_reassign_destination(SLUG, &json!("S1"), None, None)
            .expect_err("neither a station nor a stream must be refused");
        assert_eq!(err.1, 400);
    }

    #[test]
    fn do_reassign_destination_assigns_both_a_station_and_a_stream() {
        // Same both-at-once rule as do_start_match: a set can sit at a
        // physical station AND on a stream, so requesting both assigns both.
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", Value::Null)],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(2), Some("socalrivals".to_string()))
            .expect("a station and a stream at once assigns both");
        assert_eq!(res["ok"], json!(true));
        assert_eq!(
            fake.assign_calls(),
            vec![(json!("S1"), json!("opaque-st-2"))]
        );
        assert_eq!(
            fake.assign_stream_calls(),
            vec![(json!("S1"), json!("opaque-stream-1"))]
        );
        assert!(fake.start_calls().is_empty());
    }

    #[test]
    fn do_reassign_destination_skips_an_unchanged_destination() {
        // Already at station 2; only the stream actually changes, so only
        // the stream is assigned -- no redundant assign_station mutation.
        let fake = FakeStartgg::new();
        fake.set_available_sets(json!({
            "sets": [set_with_station("S1", json!(2))],
            "stations": stations_list(),
            "streams": streams_list(),
        }));
        let mut h = Hub::new(None, None, None, None, None, None, None, None);
        h.startgg = Box::new(Shared(fake.clone()));

        let res = h
            .do_reassign_destination(SLUG, &json!("S1"), Some(2), Some("socalrivals".to_string()))
            .expect("an unchanged station alongside a new stream succeeds");
        assert_eq!(res["ok"], json!(true));
        assert!(
            fake.assign_calls().is_empty(),
            "already on station 2 -> no redundant assign_station call"
        );
        assert_eq!(
            fake.assign_stream_calls(),
            vec![(json!("S1"), json!("opaque-stream-1"))]
        );
    }
}
