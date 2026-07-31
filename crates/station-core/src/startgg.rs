//! start.gg client for the LAN hub.
//!
//! Only the operator runs this: the API token lives on exactly one machine and
//! every start.gg call for the event funnels through one rate-limited client.
//!
//! The broker had to read through start.gg's unauthenticated website API (it had
//! no token for reads); here we hold a token, so reads and writes both go to the
//! official endpoint.
//!
//! Write semantics, unchanged from the broker:
//! * `update_live()`  -> markSetInProgress + updateBracketSet: records the
//!   games-so-far WITHOUT a winner, so the bracket does not
//!   advance. Safe to call automatically after every game.
//! * `report_set()`   -> reportBracketSet with a winner: advances the bracket.
//!   Only ever called from an explicit operator action.
//!
//! # Port notes (for the hub porter)
//!
//! The Python hub is tested by assigning a duck-typed fake (`h.startgg =
//! FakeStartgg()` in `test_hub.py`) that provides `enabled`, `station_set`,
//! `character_map`, `update_live`, and `report_set`. The Rust hub should hold a
//! `Box<dyn StartggApi>` (or accept one in its constructor) so tests can inject
//! a fake the same way; [`Startgg`] implements [`StartggApi`] by delegating to
//! its inherent methods. Signature choices, mirroring how `hub.py` calls in:
//!
//! * `station_set(slug, station, max_age)` — Rust has no default arguments, so
//!   `max_age` is explicit: pass [`STATION_CACHE_S`] where Python used the
//!   default and `0.0` where it passed `max_age=0` (force a fresh read).
//!   Returns `Ok(Value::Null)` where Python returned `None` (no set at the
//!   station).
//! * `character_map(slug)` — like the Python, this never raises: transport
//!   errors are logged and the stale cached map (or `{}`) is returned. It still
//!   returns `Result` for trait uniformity; the real client always returns `Ok`.
//! * `update_live(set_id, game_data)` / `report_set(set_id, winner_entrant_id,
//!   game_data)` — GraphQL IDs may arrive as JSON numbers or strings, so ids
//!   are `&Value`, and `game_data` is dynamic `Value` (a JSON array of
//!   `BracketSetGameDataInput` objects), never a typed struct.
//!
//! `character_map` needs `matching.norm`; `matching.rs` is still a stub, so a
//! private byte-equivalent copy lives here (see [`norm`]). Once the matching
//! module is ported, this can switch to `crate::matching::norm` (same
//! behaviour: lowercase, strip everything that isn't `a-z0-9`).

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

pub const API_URL: &str = "https://api.start.gg/gql/alpha";
// start.gg allows ~80 requests/60s per token; stay well under it.
pub const MIN_INTERVAL_S: f64 = 0.8;
pub const STATION_CACHE_S: f64 = 15.0; // station lookups repeat on every heartbeat
pub const CHARACTER_CACHE_S: f64 = 604800.0;

/// GraphQL query strings, byte-for-byte from the Python source.
const STATION_SET_QUERY: &str = "query($slug:String!){ event(slug:$slug){\n                 sets(page:1, perPage:60, sortType:STANDARD, filters:{ state:[1,2,6] }){\n                   nodes{ id state fullRoundText station{ number }\n                     slots{ entrant{ id name } } } } } }";
const CHARACTER_MAP_QUERY: &str =
    "query($slug:String!){ event(slug:$slug){ videogame{ id characters{ id name } } } }";
const UPDATE_LIVE_MUTATION: &str = "mutation($id:ID!,$g:[BracketSetGameDataInput]){\n                 updateBracketSet(setId:$id, gameData:$g){ id state } }";
const REPORT_SET_MUTATION: &str = "mutation($setId:ID!,$winnerId:ID!,$gameData:[BracketSetGameDataInput]){\n                 reportBracketSet(setId:$setId, winnerId:$winnerId, gameData:$gameData){ id state } }";
// `updateBracketSet`'s own response only echoes { id state }, never the game
// data it just wrote, so confirming a live push landed needs a separate read.
// Field names verified against the live schema (introspected on api.start.gg,
// not assumed): Game has orderNum/winnerId/selections, GameSelection has
// entrant{id}/character{id}. `set(id:)` is a direct root lookup, cheaper than
// re-listing the whole event's sets for the one we already know the id of.
const SET_GAMES_QUERY: &str = "query($setId:ID!){ set(id:$setId){ games{ orderNum winnerId\n                 selections{ entrant{ id } character{ id } } } } }";
// A cheap direct-by-id state check, bypassing STATION_SET_QUERY's [1,2,6]
// filter (which is exactly the problem: once a set reaches state 3 it drops
// out of that list silently, so a filtered lookup can't tell "already
// completed" apart from "gone missing"). do_report uses this to refuse a
// write rather than attempt one against a set someone already finalized
// through start.gg's own page.
const SET_STATE_QUERY: &str = "query($setId:ID!){ set(id:$setId){ state } }";
// Sets available to start (state 1 = created, 6 = called-not-started; state
// 2/ongoing and 3/completed are deliberately excluded -- those aren't
// "available to start" anymore), plus the event's stations in the same
// round trip: the Start Match panel needs both, and fetching stations
// separately for every render would be a second full request for data that
// changes only when a TO adds/removes a physical setup. `Stations.id` is an
// opaque GraphQL id; `Stations.number` is the plain integer (1, 2, 3...)
// this app already uses for its own station numbering (`cfg.station`) --
// assigning a set to "station 2" means resolving that number to this id
// first, never passing the number itself to `assignStation`.
const AVAILABLE_SETS_QUERY: &str = "query($slug:String!){ event(slug:$slug){\n                 sets(page:1, perPage:60, sortType:STANDARD, filters:{state:[1,6]}){\n                   nodes{ id state fullRoundText station{ number }\n                     slots{ entrant{ id name } } } }\n                 stations(query:{perPage:32}){ nodes{ id number enabled } } } }";
// `assignStation`'s own required args, confirmed against the live schema:
// just the set and the station's opaque id.
const ASSIGN_STATION_MUTATION: &str =
    "mutation($setId:ID!,$stationId:ID!){ assignStation(setId:$setId, stationId:$stationId){ id } }";
// The TO's call ("Start Match" on start.gg) -- never invoked automatically
// anywhere else in this app; only this explicit operator-clicked path (see
// hub.rs's `do_start_match`) is allowed to call it.
const START_MATCH_MUTATION: &str =
    "mutation($setId:ID!){ markSetInProgress(setId:$setId){ id state } }";

/// Error type mirroring Python's `StartggError(Exception)`: just a message.
#[derive(Debug, Clone)]
pub struct StartggError(pub String);

impl fmt::Display for StartggError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StartggError {}

/// The interface the hub consumes; lets tests inject a fake (the Rust
/// counterpart of `h.startgg = FakeStartgg()` in `test_hub.py`).
pub trait StartggApi: Send + Sync {
    fn enabled(&self) -> bool;
    fn station_set(&self, slug: &str, station: i64, max_age: f64) -> Result<Value, StartggError>;
    fn character_map(&self, slug: &str) -> Result<Value, StartggError>;
    fn update_live(&self, set_id: &Value, game_data: &Value) -> Result<(), StartggError>;
    /// Read back what start.gg actually has for a set's games, to confirm a
    /// prior `update_live` landed. Returns the raw `games` field (an array,
    /// or `Value::Null` if the set has none yet); [`live_push_confirmed`]
    /// does the comparison against what was pushed.
    fn set_games(&self, set_id: &Value) -> Result<Value, StartggError>;
    /// The set's current state on start.gg (raw `Set.state`, or `Value::Null`
    /// if the set can't be found at all), read directly by id. Used to catch
    /// a set someone finalized through start.gg's own page before this hub's
    /// Report button gets to it -- `STATION_SET_QUERY`'s filtered list can't
    /// tell that apart from "gone missing" once a set reaches state 3.
    fn set_state(&self, set_id: &Value) -> Result<Value, StartggError>;
    fn report_set(
        &self,
        set_id: &Value,
        winner_entrant_id: &Value,
        game_data: Option<&Value>,
    ) -> Result<Value, StartggError>;
    /// Sets available to start (both entrants determined, not yet begun) plus
    /// the event's stations. See [`Startgg::available_sets`] for the exact
    /// normalized shape.
    fn available_sets(&self, slug: &str) -> Result<Value, StartggError>;
    /// Assign a set to a station. `station_id` must be the station's opaque
    /// GraphQL id (resolved from its plain number by the caller), never the
    /// number itself.
    fn assign_station(&self, set_id: &Value, station_id: &Value) -> Result<(), StartggError>;
    /// `markSetInProgress` -- the TO's "Start Match" button. Only ever called
    /// from an explicit operator action; never automatic.
    fn start_match(&self, set_id: &Value) -> Result<(), StartggError>;
}

type LogFn = Box<dyn Fn(&str) + Send + Sync>;

pub struct Startgg {
    token: String,
    log: LogFn,
    /// Serializes + throttles requests (Python held one `threading.Lock`
    /// around the gap check *and* the sleep, so concurrent callers queue).
    last_call: Mutex<f64>,
    /// slug -> (fetched_at, {norm(name): id})
    chars: Mutex<HashMap<String, (f64, Value)>>,
    /// (slug, station) -> (fetched_at, result); result is Null for "no set".
    stations: Mutex<HashMap<(String, i64), (f64, Value)>>,
    client: reqwest::blocking::Client,
}

impl Startgg {
    pub fn new(token: Option<String>) -> Self {
        Self::with_log(token, None)
    }

    /// Python's `Startgg(token, log=...)`; `new` is the `log=None` case.
    pub fn with_log(token: Option<String>, log: Option<LogFn>) -> Self {
        Startgg {
            token: token.unwrap_or_default().trim().to_string(),
            log: log.unwrap_or_else(|| Box::new(|_m| {})),
            last_call: Mutex::new(0.0),
            chars: Mutex::new(HashMap::new()),
            stations: Mutex::new(HashMap::new()),
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.token.is_empty()
    }

    // -- transport ----------------------------------------------------------
    fn gql(&self, query: &str, variables: Value) -> Result<Value, StartggError> {
        if self.token.is_empty() {
            return Err(StartggError("no start.gg token configured".to_string()));
        }
        {
            // serialize + throttle (lock is held across the sleep, as in Python)
            let mut last = self.last_call.lock().unwrap();
            let gap = now() - *last;
            if gap < MIN_INTERVAL_S {
                std::thread::sleep(Duration::from_secs_f64(MIN_INTERVAL_S - gap));
            }
            *last = now();
        }
        let body = json!({ "query": query, "variables": variables });
        let resp = self
            .client
            .post(API_URL)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "rivals-station-hub/1.0")
            .json(&body)
            .send()
            .map_err(|e| StartggError(format!("start.gg unreachable: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            // urllib raises HTTPError for any non-2xx; mirror the message.
            return Err(StartggError(format!("start.gg HTTP {}", status.as_u16())));
        }
        let out: Value = resp
            .json()
            .map_err(|_| StartggError("start.gg returned invalid JSON".to_string()))?;
        if truthy(out.get("errors")) {
            let msg = match out
                .get("errors")
                .and_then(|e| e.get(0))
                .and_then(|e0| e0.get("message"))
            {
                Some(m) if truthy(Some(m)) => match m {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
                _ => "GraphQL error".to_string(),
            };
            return Err(StartggError(msg));
        }
        Ok(match out.get("data") {
            Some(d) if truthy(Some(d)) => d.clone(),
            _ => json!({}),
        })
    }

    // -- reads --------------------------------------------------------------
    /// The not-yet-completed set currently at a station, with entrants.
    ///
    /// start.gg's filters don't reliably support station number, so pull the
    /// event's active sets and match locally (same approach as the broker).
    pub fn station_set(
        &self,
        slug: &str,
        station: i64,
        max_age: f64,
    ) -> Result<Value, StartggError> {
        let key = (slug.to_string(), station);
        if let Some((fetched_at, cached)) = self.stations.lock().unwrap().get(&key) {
            if now() - fetched_at < max_age {
                return Ok(cached.clone());
            }
        }
        let data = self.gql(STATION_SET_QUERY, json!({ "slug": slug }))?;
        let empty = Vec::new();
        let nodes = data
            .get("event")
            .and_then(|e| e.get("sets"))
            .and_then(|s| s.get("nodes"))
            .and_then(|n| n.as_array())
            .unwrap_or(&empty);
        let mut found: Option<&Value> = None;
        for n in nodes {
            let st = n.get("station").and_then(|s| s.get("number"));
            if let Some(st_num) = st.and_then(as_int) {
                if st_num == station {
                    found = Some(n);
                    break;
                }
            }
        }
        let result = match found {
            Some(f) => {
                let mut entrants: Vec<Value> = Vec::new();
                if let Some(slots) = f.get("slots").and_then(|s| s.as_array()) {
                    for s in slots {
                        if let Some(e) = s.get("entrant").filter(|e| truthy(Some(e))) {
                            entrants.push(json!({
                                "id": e.get("id").cloned().unwrap_or(Value::Null),
                                "name": or_empty_string(e.get("name")),
                            }));
                        }
                    }
                }
                json!({
                    "found": true,
                    "setId": f.get("id").cloned().unwrap_or(Value::Null),
                    "state": f.get("state").cloned().unwrap_or(Value::Null),
                    "fullRoundText": or_empty_string(f.get("fullRoundText")),
                    "entrants": entrants,
                })
            }
            None => Value::Null,
        };
        self.stations
            .lock()
            .unwrap()
            .insert(key, (now(), result.clone()));
        Ok(result)
    }

    /// Character name -> start.gg id for the event's game (cached).
    ///
    /// Never errors: like the Python, a failed fetch is logged and the stale
    /// cached map (or an empty map) is returned.
    pub fn character_map(&self, slug: &str) -> Result<Value, StartggError> {
        let stale: Option<Value> = {
            let chars = self.chars.lock().unwrap();
            match chars.get(slug) {
                Some((fetched_at, cached)) => {
                    if now() - fetched_at < CHARACTER_CACHE_S {
                        return Ok(cached.clone());
                    }
                    Some(cached.clone())
                }
                None => None,
            }
        };
        let data = match self.gql(CHARACTER_MAP_QUERY, json!({ "slug": slug })) {
            Ok(d) => d,
            Err(e) => {
                (self.log)(&format!("character map unavailable: {e}"));
                return Ok(stale.unwrap_or_else(|| json!({})));
            }
        };
        let empty = Vec::new();
        let chars = data
            .get("event")
            .and_then(|e| e.get("videogame"))
            .and_then(|v| v.get("characters"))
            .and_then(|c| c.as_array())
            .unwrap_or(&empty);
        let mut cmap = serde_json::Map::new();
        for c in chars {
            if truthy(Some(c)) {
                if let Some(name) = c.get("name").filter(|n| !n.is_null()) {
                    let key = norm(&value_str(name));
                    cmap.insert(key, c.get("id").cloned().unwrap_or(Value::Null));
                }
            }
        }
        let cmap = Value::Object(cmap);
        if truthy(Some(&cmap)) {
            self.chars
                .lock()
                .unwrap()
                .insert(slug.to_string(), (now(), cmap.clone()));
        }
        Ok(cmap)
    }

    // -- writes -------------------------------------------------------------
    /// Non-advancing: record games so far. No winner is set, so the bracket
    /// never advances — finalizing stays an explicit operator action.
    ///
    /// Deliberately does NOT call markSetInProgress: starting a match is the
    /// TO's call ("Start Match" on start.gg), and callers only reach here for
    /// a set that is already ongoing. Doing it here meant warmups at a setup
    /// could start the bracket match on their own.
    pub fn update_live(&self, set_id: &Value, game_data: &Value) -> Result<(), StartggError> {
        self.gql(
            UPDATE_LIVE_MUTATION,
            json!({ "id": set_id, "g": game_data }),
        )?;
        Ok(())
    }

    /// See [`StartggApi::set_games`].
    pub fn set_games(&self, set_id: &Value) -> Result<Value, StartggError> {
        let data = self.gql(SET_GAMES_QUERY, json!({ "setId": set_id }))?;
        Ok(data
            .get("set")
            .and_then(|s| s.get("games"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// See [`StartggApi::set_state`].
    pub fn set_state(&self, set_id: &Value) -> Result<Value, StartggError> {
        let data = self.gql(SET_STATE_QUERY, json!({ "setId": set_id }))?;
        Ok(data
            .get("set")
            .and_then(|s| s.get("state"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Advancing: set the winner and finalize. Operator action only.
    pub fn report_set(
        &self,
        set_id: &Value,
        winner_entrant_id: &Value,
        game_data: Option<&Value>,
    ) -> Result<Value, StartggError> {
        self.gql(
            REPORT_SET_MUTATION,
            json!({
                "setId": set_id,
                "winnerId": winner_entrant_id,
                "gameData": game_data.cloned().unwrap_or(Value::Null),
            }),
        )
    }

    /// Sets available to start on this event, plus its stations.
    ///
    /// Returns `{ "sets": [{id, fullRoundText, station, entrants}], "stations":
    /// [{id, number}] }`. `station` is the plain station number (or `Null` if
    /// unassigned); `entrants` is always exactly two `{id, name}` objects --
    /// a set with either slot still undetermined (waiting on a prior round,
    /// e.g. `slots: [{entrant: null}, {entrant: null}]` for a not-yet-seeded
    /// Grand Final) is left out entirely, never included with a null/missing
    /// entrant. `stations[].id` is the opaque id `assign_station` needs;
    /// `stations[].number` is the plain integer this app's own station
    /// numbering already uses.
    pub fn available_sets(&self, slug: &str) -> Result<Value, StartggError> {
        let data = self.gql(AVAILABLE_SETS_QUERY, json!({ "slug": slug }))?;
        Ok(parse_available_sets(&data))
    }

    /// See [`StartggApi::assign_station`].
    pub fn assign_station(&self, set_id: &Value, station_id: &Value) -> Result<(), StartggError> {
        self.gql(
            ASSIGN_STATION_MUTATION,
            json!({ "setId": set_id, "stationId": station_id }),
        )?;
        Ok(())
    }

    /// See [`StartggApi::start_match`].
    pub fn start_match(&self, set_id: &Value) -> Result<(), StartggError> {
        self.gql(START_MATCH_MUTATION, json!({ "setId": set_id }))?;
        Ok(())
    }

    // -- test hooks ----------------------------------------------------------
    #[cfg(test)]
    fn seed_station_cache(&self, slug: &str, station: i64, fetched_at: f64, result: Value) {
        self.stations
            .lock()
            .unwrap()
            .insert((slug.to_string(), station), (fetched_at, result));
    }

    #[cfg(test)]
    fn seed_character_cache(&self, slug: &str, fetched_at: f64, map: Value) {
        self.chars
            .lock()
            .unwrap()
            .insert(slug.to_string(), (fetched_at, map));
    }
}

impl StartggApi for Startgg {
    fn enabled(&self) -> bool {
        Startgg::enabled(self)
    }
    fn station_set(&self, slug: &str, station: i64, max_age: f64) -> Result<Value, StartggError> {
        Startgg::station_set(self, slug, station, max_age)
    }
    fn character_map(&self, slug: &str) -> Result<Value, StartggError> {
        Startgg::character_map(self, slug)
    }
    fn update_live(&self, set_id: &Value, game_data: &Value) -> Result<(), StartggError> {
        Startgg::update_live(self, set_id, game_data)
    }
    fn set_games(&self, set_id: &Value) -> Result<Value, StartggError> {
        Startgg::set_games(self, set_id)
    }
    fn set_state(&self, set_id: &Value) -> Result<Value, StartggError> {
        Startgg::set_state(self, set_id)
    }
    fn report_set(
        &self,
        set_id: &Value,
        winner_entrant_id: &Value,
        game_data: Option<&Value>,
    ) -> Result<Value, StartggError> {
        Startgg::report_set(self, set_id, winner_entrant_id, game_data)
    }
    fn available_sets(&self, slug: &str) -> Result<Value, StartggError> {
        Startgg::available_sets(self, slug)
    }
    fn assign_station(&self, set_id: &Value, station_id: &Value) -> Result<(), StartggError> {
        Startgg::assign_station(self, set_id, station_id)
    }
    fn start_match(&self, set_id: &Value) -> Result<(), StartggError> {
        Startgg::start_match(self, set_id)
    }
}

/// Parsing half of [`Startgg::available_sets`], split out from the network
/// call so it's testable against a hand-built GraphQL response shape without
/// a live token.
fn parse_available_sets(data: &Value) -> Value {
    let empty = Vec::new();
    let nodes = data
        .get("event")
        .and_then(|e| e.get("sets"))
        .and_then(|s| s.get("nodes"))
        .and_then(|n| n.as_array())
        .unwrap_or(&empty);
    let mut sets: Vec<Value> = Vec::new();
    for n in nodes {
        let slots = match n.get("slots").and_then(|s| s.as_array()) {
            Some(a) if a.len() == 2 => a,
            _ => continue,
        };
        let mut entrants: Vec<Value> = Vec::new();
        for s in slots {
            match s.get("entrant").filter(|e| truthy(Some(e))) {
                Some(e) => entrants.push(json!({
                    "id": e.get("id").cloned().unwrap_or(Value::Null),
                    "name": or_empty_string(e.get("name")),
                })),
                // An undetermined slot (waiting on a prior round) -- the
                // whole set is left out, never included with a null entrant.
                None => break,
            }
        }
        if entrants.len() != 2 {
            continue;
        }
        sets.push(json!({
            "id": n.get("id").cloned().unwrap_or(Value::Null),
            "fullRoundText": or_empty_string(n.get("fullRoundText")),
            "station": n
                .get("station")
                .and_then(|s| s.get("number"))
                .cloned()
                .unwrap_or(Value::Null),
            "entrants": entrants,
        }));
    }
    let station_nodes = data
        .get("event")
        .and_then(|e| e.get("stations"))
        .and_then(|s| s.get("nodes"))
        .and_then(|n| n.as_array())
        .unwrap_or(&empty);
    let stations: Vec<Value> = station_nodes
        .iter()
        .map(|s| {
            json!({
                "id": s.get("id").cloned().unwrap_or(Value::Null),
                "number": s.get("number").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({ "sets": sets, "stations": stations })
}

// -- helpers ------------------------------------------------------------------

/// `time.time()` — seconds since the epoch as f64.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Python truthiness for a JSON value (`if x:`); `None` (absent key) is falsy.
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

/// Python `x or ''`: keep a truthy value as-is, anything falsy becomes `""`.
fn or_empty_string(v: Option<&Value>) -> Value {
    match v {
        Some(val) if truthy(Some(val)) => val.clone(),
        _ => Value::String(String::new()),
    }
}

/// Python `int(x)` for the shapes start.gg actually returns (number or
/// numeric string); `None`/other shapes yield `None` (the caller skips them).
fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Python `str(x)` for the values we feed to `norm` (character names).
fn value_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Local mirror of `matching.norm` (matching.rs is not ported yet):
/// lowercase, strip everything that isn't a letter or digit.
fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_reflects_token() {
        assert!(!Startgg::new(None).enabled());
        assert!(!Startgg::new(Some(String::new())).enabled());
        assert!(!Startgg::new(Some("   ".to_string())).enabled()); // Python strips
        assert!(Startgg::new(Some("tok".to_string())).enabled());
        assert!(Startgg::new(Some("  tok  ".to_string())).enabled());
    }

    #[test]
    fn error_displays_its_message() {
        let e = StartggError("start.gg HTTP 429".to_string());
        assert_eq!(e.to_string(), "start.gg HTTP 429");
        let dyn_err: &dyn std::error::Error = &e;
        assert_eq!(format!("{dyn_err}"), "start.gg HTTP 429");
    }

    #[test]
    fn station_cache_serves_fresh_entries_without_network() {
        // No token: any path that reaches gql() errors, so an Ok proves the
        // cache answered.
        let sg = Startgg::new(None);
        let cached = json!({
            "found": true, "setId": 105639152, "state": 2,
            "fullRoundText": "Winners Round 1", "entrants": []
        });
        sg.seed_station_cache("slug", 1, now(), cached.clone());
        assert_eq!(sg.station_set("slug", 1, STATION_CACHE_S).unwrap(), cached);

        // A cached "no set at this station" (Python None) is served too.
        sg.seed_station_cache("slug", 2, now(), Value::Null);
        assert!(sg
            .station_set("slug", 2, STATION_CACHE_S)
            .unwrap()
            .is_null());

        // Stale entry falls through to the network path (-> token error here).
        sg.seed_station_cache("slug", 3, now() - 100.0, cached.clone());
        let err = sg.station_set("slug", 3, STATION_CACHE_S).unwrap_err();
        assert!(err.to_string().contains("no start.gg token"));

        // max_age=0 forces a re-read even with a fresh entry (hub uses this).
        let err = sg.station_set("slug", 1, 0.0).unwrap_err();
        assert!(err.to_string().contains("no start.gg token"));
    }

    #[test]
    fn character_cache_serves_fresh_and_falls_back_to_stale() {
        use std::sync::{Arc, Mutex};
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = logs.clone();
        let sg = Startgg::with_log(
            None,
            Some(Box::new(move |m| sink.lock().unwrap().push(m.to_string()))),
        );
        let cmap = json!({"orcane": 41, "galvan": 42});

        // Fresh entry: returned as-is, no network, no log.
        sg.seed_character_cache("slug", now(), cmap.clone());
        assert_eq!(sg.character_map("slug").unwrap(), cmap);
        assert!(logs.lock().unwrap().is_empty());

        // Stale entry + failing fetch (no token): logs and returns the stale map.
        sg.seed_character_cache("slug", now() - CHARACTER_CACHE_S - 1.0, cmap.clone());
        assert_eq!(sg.character_map("slug").unwrap(), cmap);
        {
            let l = logs.lock().unwrap();
            assert_eq!(l.len(), 1);
            assert!(l[0].starts_with("character map unavailable:"));
        }

        // No cache at all + failing fetch: logs and returns an empty map.
        assert_eq!(sg.character_map("other-slug").unwrap(), json!({}));
        assert_eq!(logs.lock().unwrap().len(), 2);
    }

    #[test]
    fn startgg_is_usable_as_a_trait_object() {
        let sg: Box<dyn StartggApi> = Box::new(Startgg::new(Some("tok".to_string())));
        assert!(sg.enabled());
    }

    #[test]
    fn norm_matches_the_python_helper() {
        assert_eq!(norm("JUGZ!"), "jugz");
        assert_eq!(norm("Mr. Game & Watch 2"), "mrgamewatch2");
        assert_eq!(norm(""), "");
    }

    /// Real shapes confirmed live: a ready set has `entrant:{id,name}` on
    /// both slots; a set waiting on a prior round (Losers Final, Grand
    /// Final) has `entrant: null` on the undetermined side(s). Only sets
    /// with both slots determined should survive.
    #[test]
    fn parse_available_sets_only_keeps_sets_with_two_determined_entrants() {
        let data = json!({
            "event": {
                "sets": { "nodes": [
                    {
                        "id": "set-1", "state": 1, "fullRoundText": "Winners Quarter-Final",
                        "station": {"number": 2},
                        "slots": [
                            {"entrant": {"id": "E1", "name": "jugeeya"}},
                            {"entrant": {"id": "E2", "name": "Kimchi"}},
                        ],
                    },
                    {
                        "id": "set-2", "state": 1, "fullRoundText": "Losers Final",
                        "station": Value::Null,
                        "slots": [{"entrant": Value::Null}, {"entrant": Value::Null}],
                    },
                    {
                        "id": "set-3", "state": 6, "fullRoundText": "Grand Final",
                        "station": Value::Null,
                        "slots": [
                            {"entrant": {"id": "E3", "name": "Brujita"}},
                            {"entrant": Value::Null},
                        ],
                    },
                ]},
                "stations": { "nodes": [
                    {"id": "opaque-st-1", "number": 1, "enabled": true},
                    {"id": "opaque-st-2", "number": 2, "enabled": true},
                ]},
            }
        });

        let out = parse_available_sets(&data);
        let sets = out["sets"].as_array().unwrap();
        assert_eq!(
            sets.len(),
            1,
            "only the set with two determined entrants survives  [{sets:?}]"
        );
        assert_eq!(sets[0]["id"], json!("set-1"));
        assert_eq!(sets[0]["fullRoundText"], json!("Winners Quarter-Final"));
        assert_eq!(sets[0]["station"], json!(2), "station is the plain number");
        assert_eq!(
            sets[0]["entrants"],
            json!([{"id": "E1", "name": "jugeeya"}, {"id": "E2", "name": "Kimchi"}])
        );

        let stations = out["stations"].as_array().unwrap();
        assert_eq!(stations.len(), 2);
        assert_eq!(
            stations[0],
            json!({"id": "opaque-st-1", "number": 1}),
            "stations carry the opaque id alongside the plain number"
        );
    }

    #[test]
    fn parse_available_sets_handles_a_set_with_no_station_assigned() {
        let data = json!({
            "event": {
                "sets": { "nodes": [
                    {
                        "id": "set-4", "state": 6, "fullRoundText": "Losers Round 3",
                        "station": Value::Null,
                        "slots": [
                            {"entrant": {"id": "E5", "name": "Loom"}},
                            {"entrant": {"id": "E6", "name": "Rando"}},
                        ],
                    },
                ]},
                "stations": { "nodes": [] },
            }
        });
        let out = parse_available_sets(&data);
        assert_eq!(out["sets"][0]["station"], Value::Null);
    }
}
