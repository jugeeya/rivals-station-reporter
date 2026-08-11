//! Set/entrant matching for the LAN hub — a port of the logic in
//! broker/worker.js (via the Python `matching.py`), so the operator can run an
//! event with no Cloudflare at all.
//!
//! Pure functions over `serde_json::Value` (the hub and the station sender both
//! pass dynamic JSON records across every boundary). Kept deliberately close to
//! the Worker's behaviour so a set looks the same whether it went through the
//! broker or the hub.
//!
//! One intentional difference: the Worker's winner match normalised names by
//! stripping whitespace only, while slot mapping stripped every non-alphanumeric.
//! Here both use the stricter [`norm`], so "JUGZ!" and "jugz" compare equal — that
//! only ever makes matching more forgiving, which is the direction we want.
//!
//! Deliberate extensions beyond the Worker, each earned by a real set from
//! The Hangout 4.1 (see the `hangout41` test module):
//! * [`norm_key`]: a symbols-only tag like `***` normalises to nothing under
//!   [`norm`], which made it unmatchable AND undeliverable through the tag
//!   database (its key was dropped). Such tags fall back to a
//!   lowercased/whitespace-stripped form instead of vanishing.
//! * Elimination: in a two-entrant set, when the winner's tag is unmatchable
//!   but the LOSER's tag matches an entrant exactly, the winner is the other
//!   entrant by elimination — as certain as a direct exact match. (`SHLNG`
//!   won four sets including grands; the tag database knew every opponent.)
//! * Abbreviation: `SMSC` is how Samasicus tags in game. A short tag that
//!   anchors on the same first letter and reads as an ordered subsequence of
//!   exactly one entrant's name is a "low"-confidence candidate.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

/// start.gg Set.state:
///   1 = created (seeded, not called)   2 = ongoing — the TO pressed "Start Match"
///   3 = completed                      6 = called to a station, still not started
/// Only 2 means the match is genuinely underway. Until then a station's games are
/// warmups/friendlies at that setup, so they must not be bound to the bracket set
/// or pushed to it — and we never start a match ourselves.
pub const STARTGG_STATE_ONGOING: i64 = 2;
/// A set that reached this state was reported by someone or something other
/// than this hub's own `do_report` (that call marks our record `reported`
/// itself, so if this shows up on a set we still think is merely
/// "awaiting report", start.gg's is the version of events that happened).
pub const STARTGG_STATE_COMPLETED: i64 = 3;

/// Python truthiness for a JSON value (`None`/`False`/`0`/`""`/`[]`/`{}` are falsy).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn truthy_opt(v: Option<&Value>) -> bool {
    v.is_some_and(truthy)
}

/// Python `str()` over a JSON value: strings pass through unquoted, numbers
/// render as written, `null`/`true`/`false` render the Python way.
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

/// Python `int()` over a JSON value; `None` where the original raised
/// TypeError/ValueError (missing/null, unparsable string, non-scalar).
fn py_int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// True once the TO has actually started this set on start.gg.
pub fn set_started(startgg_set: Option<&Value>) -> bool {
    let sg = match startgg_set {
        Some(v) if truthy(v) => v,
        _ => return false,
    };
    match py_int(sg.get("state")) {
        Some(state) => state == STARTGG_STATE_ONGOING,
        None => false,
    }
}

/// Lowercase, strip everything that isn't a letter or digit.
pub fn norm(s: &Value) -> String {
    if s.is_null() {
        return String::new();
    }
    norm_str(&py_str(s))
}

fn norm_str(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// [`norm_str`], except a name that would normalise to NOTHING (a
/// symbols-only tag like `***` or `:)`) falls back to its lowercased,
/// whitespace-stripped form instead. Alphanumeric names key exactly as
/// before; symbol tags stop being invisible to every lookup.
pub fn norm_key(s: &str) -> String {
    let n = norm_str(s);
    if !n.is_empty() {
        return n;
    }
    s.to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// True when `tag` reads as an abbreviation of `name`: at least 3
/// characters, strictly shorter, anchored on the same first character, and
/// every character appears in `name` in order ("smsc" ⊂ "samasicus",
/// "altn" ⊂ "altonp"). Both inputs are already normalised.
fn is_abbreviation(tag: &str, name: &str) -> bool {
    if tag.chars().count() < 3 || tag.len() >= name.len() {
        return false;
    }
    let mut want = tag.chars();
    let mut cur = want.next();
    if cur != name.chars().next() {
        return false;
    }
    for c in name.chars() {
        if Some(c) == cur {
            cur = want.next();
            if cur.is_none() {
                return true;
            }
        }
    }
    false
}

/// Every name an entrant may appear under: start.gg tags can carry a
/// sponsor prefix ("TEAM | Tag"), so match the full name and the bare tag.
pub fn entrant_names(e: &Value) -> Vec<String> {
    let full = match e.get("name") {
        Some(n) if truthy(n) => py_str(n),
        _ => String::new(),
    };
    if full.contains('|') {
        let bare = full.split('|').next_back().unwrap_or("").trim().to_string();
        if !bare.is_empty() {
            return vec![full, bare];
        }
    }
    vec![full]
}

/// players.json is written by hand as {"SAVE TAG": "startgg tag"}; lookups
/// happen on normalized names, so key it that way once at load.
pub fn build_tag_map(players_json: Option<&Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(obj) = players_json.and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if k.starts_with('_') {
                continue; // "_comment" style notes people leave in the file
            }
            let nk = norm_key(k);
            if nk.is_empty() {
                continue;
            }
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    out.insert(nk, s.to_string());
                }
            }
        }
    }
    out
}

/// Every name a player might appear under on start.gg: an explicit
/// start.gg name from the station (`sgg`), the players.json translation of
/// their save tag, then the raw in-game name.
pub fn player_aliases(p: Option<&Value>, tag_map: Option<&HashMap<String, String>>) -> Vec<String> {
    let mut out = Vec::new();
    let p = match p {
        Some(v) if truthy(v) => v,
        _ => return out,
    };
    if truthy_opt(p.get("sgg")) {
        out.push(py_str(p.get("sgg").unwrap()));
    }
    if let Some(tm) = tag_map {
        if let Some(name) = p.get("name").filter(|v| truthy(v)) {
            if let Some(via) = tm.get(&norm_key(&py_str(name))) {
                if !via.is_empty() {
                    out.push(via.clone());
                }
            }
        }
    }
    if truthy_opt(p.get("name")) {
        out.push(py_str(p.get("name").unwrap()));
    }
    out
}

/// Result of [`match_winner`]: the candidate entrant id (raw JSON value, as
/// start.gg sent it) and a confidence label ("high" / "low" / "none").
#[derive(Debug, Clone, PartialEq)]
pub struct WinnerMatch {
    pub candidate_winner_entrant_id: Option<Value>,
    pub confidence: &'static str,
}

/// A player's normalised aliases, ready for comparison.
fn norm_aliases(p: Option<&Value>, tag_map: Option<&HashMap<String, String>>) -> Vec<String> {
    player_aliases(p, tag_map)
        .iter()
        .map(|a| norm_key(a))
        .filter(|a| !a.is_empty())
        .collect()
}

/// How strongly a set of aliases points at one entrant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Hit {
    None,
    /// Substring either way, or an abbreviation (see [`is_abbreviation`]).
    Partial,
    /// A normalised alias equals a normalised entrant name.
    Exact,
}

fn hit_for(aliases: &[String], e: &Value) -> Hit {
    let mut best = Hit::None;
    for n in entrant_names(e) {
        let nn = norm_key(&n);
        if nn.is_empty() {
            continue;
        }
        if aliases.iter().any(|a| a == &nn) {
            return Hit::Exact;
        }
        if aliases
            .iter()
            .any(|a| nn.contains(a.as_str()) || a.contains(&nn) || is_abbreviation(a, &nn))
        {
            best = Hit::Partial;
        }
    }
    best
}

fn id_of(e: &Value) -> Option<Value> {
    e.get("id").filter(|v| !v.is_null()).cloned()
}

/// Best-effort winner match -> candidate entrant id + confidence.
///
/// In-game names rarely equal start.gg tags, so an unsure result is expected —
/// the operator confirms (or auto-report double-checks) before anything is
/// finalized. Tiers, strongest first:
///
///   1. the winner's tag matches an entrant exactly            -> high
///   2. two entrants, the LOSER's tag matches exactly: the
///      winner is the other entrant by elimination             -> high
///   3. two entrants, and the winner's and loser's partial
///      guesses each uniquely fit a DIFFERENT entrant: two
///      independent guesses corroborating each other           -> high
///   4. the winner's tag partially matches exactly one way     -> low
///   5. two entrants, the loser partially matches: elimination -> low
///
/// "high" therefore means "certain enough to advance a bracket unwatched":
/// either an exact alias somewhere in the pair, or two independent partial
/// reads that agree on who is who.
pub fn match_winner(
    st: &Value,
    entrants: Option<&Value>,
    tag_map: Option<&HashMap<String, String>>,
) -> WinnerMatch {
    let none = WinnerMatch {
        candidate_winner_entrant_id: None,
        confidence: "none",
    };
    let entrants_list = match entrants.and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => return none,
    };
    if !truthy(st) || !truthy_opt(st.get("winnerName")) {
        return none;
    }
    let winner_name = st.get("winnerName").unwrap();

    let empty: Vec<Value> = Vec::new();
    let players = st
        .get("players")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let winner_player = players.iter().find(|p| p.get("name") == Some(winner_name));
    // The other player of a 1v1 — the elimination tiers reason about them.
    let loser_player = (players.len() == 2)
        .then(|| players.iter().find(|p| p.get("name") != Some(winner_name)))
        .flatten();

    let fallback = json!({ "name": winner_name });
    let winner_aliases = norm_aliases(Some(winner_player.unwrap_or(&fallback)), tag_map);
    let loser_aliases = loser_player
        .map(|p| norm_aliases(Some(p), tag_map))
        .unwrap_or_default();
    if winner_aliases.is_empty() && loser_aliases.is_empty() {
        return none;
    }

    let winner_hits: Vec<(Hit, &Value)> = entrants_list
        .iter()
        .map(|e| (hit_for(&winner_aliases, e), e))
        .collect();
    let best_exact = winner_hits.iter().find(|(h, _)| *h == Hit::Exact);
    if let Some((_, e)) = best_exact {
        return WinnerMatch {
            candidate_winner_entrant_id: id_of(e),
            confidence: "high",
        };
    }

    // The loser's evidence, for the elimination and corroboration tiers.
    // Only meaningful in a 1v1 against exactly two entrants.
    let loser_hits: Option<Vec<Hit>> = (entrants_list.len() == 2 && !loser_aliases.is_empty())
        .then(|| {
            entrants_list
                .iter()
                .map(|e| hit_for(&loser_aliases, e))
                .collect()
        });

    // Elimination: the loser points at one entrant strictly more strongly
    // than the other; the winner is then the other one.
    let eliminate = |min: Hit| -> Option<&Value> {
        let hits = loser_hits.as_ref()?;
        for (i, h) in hits.iter().enumerate() {
            if *h >= min && hits[1 - i] < *h {
                return Some(&entrants_list[1 - i]);
            }
        }
        None
    };

    if let Some(e) = eliminate(Hit::Exact) {
        return WinnerMatch {
            candidate_winner_entrant_id: id_of(e),
            confidence: "high",
        };
    }

    let partials: Vec<usize> = winner_hits
        .iter()
        .enumerate()
        .filter(|(_, (h, _))| *h == Hit::Partial)
        .map(|(i, _)| i)
        .collect();

    // Mutual corroboration: the winner's guess uniquely fits one entrant and
    // the loser's guess uniquely fits the OTHER. Two independent partial
    // reads agreeing on who is who is as convincing as one exact read —
    // ORB(won) vs C at The Hangout 4.1: "orb" ⊂ Orbital, "c" ⊂ coopR,
    // nothing crossed. Certain enough to auto-report.
    if let (Some(hits), &[wi]) = (loser_hits.as_ref(), partials.as_slice()) {
        if hits[1 - wi] == Hit::Partial && hits[wi] == Hit::None {
            return WinnerMatch {
                candidate_winner_entrant_id: id_of(&entrants_list[wi]),
                confidence: "high",
            };
        }
    }

    // A partial guess that fits several entrants points nowhere.
    if let &[wi] = partials.as_slice() {
        return WinnerMatch {
            candidate_winner_entrant_id: id_of(&entrants_list[wi]),
            confidence: "low",
        };
    }

    if let Some(e) = eliminate(Hit::Partial) {
        return WinnerMatch {
            candidate_winner_entrant_id: id_of(e),
            confidence: "low",
        };
    }
    none
}

/// Reduce the station's matches[] to a compact per-game record (a JSON array).
/// Each match is one game; `wins` is cumulative, so the game's winner is
/// whoever's win count ticked up that game.
pub fn derive_games(st: &Value) -> Value {
    let empty: Vec<Value> = Vec::new();
    let matches = st
        .get("matches")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    // prev_wins is keyed by the slot's JSON serialization — Python keyed the
    // dict by the raw slot value; serialization preserves that equality.
    let mut prev_wins: HashMap<String, i64> = HashMap::new();
    let mut games: Vec<Value> = Vec::new();
    for (i, m) in matches.iter().enumerate() {
        let mps = m
            .get("players")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        let mut winner_slot = Value::Null;
        for p in mps {
            let slot = p.get("slot").cloned().unwrap_or(Value::Null);
            let slot_key = slot.to_string();
            let w = p
                .get("wins")
                .filter(|v| truthy(v))
                .map(|v| py_int(Some(v)).unwrap_or(0))
                .unwrap_or(0);
            if w > *prev_wins.get(&slot_key).unwrap_or(&0) {
                winner_slot = slot;
            }
            prev_wins.insert(slot_key, w);
        }
        let chars: Vec<Value> = mps
            .iter()
            .map(|p| {
                json!({
                    "slot": p.get("slot").cloned().unwrap_or(Value::Null),
                    "character": p.get("character").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        let game_num = match m.get("index") {
            Some(v) if truthy(v) => v.clone(),
            _ => json!(i as i64 + 1),
        };
        games.push(json!({
            "gameNum": game_num,
            "winnerSlot": winner_slot,
            "chars": chars,
        }));
    }
    Value::Array(games)
}

/// The compact record the hub stores and the console renders.
pub fn summarize_set(st: &Value) -> Value {
    let players: Vec<Value> = st
        .get("players")
        .and_then(|v| v.as_array())
        .map(|ps| {
            ps.iter()
                .map(|p| {
                    json!({
                        "slot": p.get("slot").cloned().unwrap_or(Value::Null),
                        "name": p.get("name").cloned().unwrap_or(Value::Null),
                        "character": p.get("character").cloned().unwrap_or(Value::Null),
                        "wins": p.get("wins").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut match_count = st.get("matchCount").cloned().unwrap_or(Value::Null);
    if match_count.is_null() {
        if let Some(ms) = st.get("matches").and_then(|v| v.as_array()) {
            match_count = json!(ms.len());
        }
    }
    let g = |k: &str| st.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "setId": g("setId"),
        "complete": truthy_opt(st.get("complete")),
        "startEpoch": g("startEpoch"),
        "endEpoch": g("endEpoch"),
        "durationSeconds": g("durationSeconds"),
        "winsRequired": g("winsRequired"),
        "matchCount": match_count,
        "winnerSlot": g("winnerSlot"),
        "winnerName": g("winnerName"),
        "winnerCharacter": g("winnerCharacter"),
        "players": players,
        "games": derive_games(st),
    })
}

/// Map player slots -> start.gg entrant ids, or `None` if it can't be done.
///
/// - Finalizing (`winner_entrant_id` given): anchor the winner's slot to the
///   chosen entrant and the other slot to the other entrant — unambiguous.
/// - Live (`winner_entrant_id` `None`): match by name, exact aliases first, then
///   fuzzy, then pair leftovers by order. Never refuses to map: a swapped
///   guess is one console click to fix, an absent live score isn't fixable.
///
/// Keys are `i64` slots: the station always reports integer slots (0/1), and
/// the Python original keyed its dict by the raw slot value, so integer keys
/// preserve the lookups hub-side (`slot_map[p['slot']]`). A player record with
/// a missing or non-integer slot cannot be keyed and the mapping comes back
/// `None` (the Python version would have keyed it by the odd value instead —
/// only reachable with malformed station data).
pub fn map_slots_to_entrants(
    rec: &Value,
    winner_entrant_id: Option<&Value>,
    tag_map: Option<&HashMap<String, String>>,
) -> Option<HashMap<i64, String>> {
    let empty: Vec<Value> = Vec::new();
    let default_st = json!({});
    let entrants = rec
        .get("entrants")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let st = match rec.get("set") {
        Some(v) if truthy(v) => v,
        _ => &default_st,
    };
    let players = st
        .get("players")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    if entrants.len() != 2 || players.len() != 2 {
        return None;
    }
    let slots: Vec<Option<i64>> = players
        .iter()
        .map(|p| p.get("slot").and_then(|v| v.as_i64()))
        .collect();

    if let Some(wid) = winner_entrant_id {
        let wid_s = py_str(wid);
        let winner_slot = st.get("winnerSlot").and_then(|v| v.as_i64());
        let other = entrants
            .iter()
            .find(|e| py_str(e.get("id").unwrap_or(&Value::Null)) != wid_s);
        let other_slot =
            winner_slot.and_then(|ws| slots.iter().flatten().copied().find(|&s| s != ws));
        let (winner_slot, other, other_slot) = (winner_slot?, other?, other_slot?);
        let mut out = HashMap::new();
        out.insert(winner_slot, wid_s);
        out.insert(other_slot, py_str(other.get("id").unwrap_or(&Value::Null)));
        return Some(out);
    }

    let id_key = |e: &Value| e.get("id").cloned().unwrap_or(Value::Null).to_string();
    let mut mapping: HashMap<i64, String> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    for exact in [true, false] {
        for p in players {
            let slot = match p.get("slot").and_then(|v| v.as_i64()) {
                Some(s) => s,
                None => continue,
            };
            if mapping.contains_key(&slot) {
                continue;
            }
            let aliases = norm_aliases(Some(p), tag_map);
            if aliases.is_empty() {
                continue;
            }
            let mut hit: Option<&Value> = None;
            for e in entrants {
                if used.contains(&id_key(e)) {
                    continue;
                }
                for n in entrant_names(e) {
                    let nn = norm_key(&n);
                    if nn.is_empty() {
                        continue;
                    }
                    let ok = if exact {
                        aliases.iter().any(|a| a == &nn)
                    } else {
                        aliases
                            .iter()
                            .any(|a| nn.contains(a.as_str()) || a.contains(&nn))
                    };
                    if ok {
                        hit = Some(e);
                        break;
                    }
                }
                if hit.is_some() {
                    break;
                }
            }
            if let Some(e) = hit {
                mapping.insert(slot, py_str(e.get("id").unwrap_or(&Value::Null)));
                used.insert(id_key(e));
            }
        }
    }

    let open_slots: Vec<i64> = slots
        .iter()
        .flatten()
        .copied()
        .filter(|s| !mapping.contains_key(s))
        .collect();
    let open_entrants: Vec<&Value> = entrants
        .iter()
        .filter(|e| !used.contains(&id_key(e)))
        .collect();
    for (i, s) in open_slots.iter().enumerate() {
        if i < open_entrants.len() {
            mapping.insert(
                *s,
                py_str(open_entrants[i].get("id").unwrap_or(&Value::Null)),
            );
        }
    }
    if mapping.len() != 2 {
        return None;
    }
    // The operator said the station guessed identities backwards -> invert.
    if truthy_opt(rec.get("swap")) {
        let (a, b) = (slots[0]?, slots[1]?);
        if let (Some(va), Some(vb)) = (mapping.get(&a).cloned(), mapping.get(&b).cloned()) {
            mapping.insert(a, vb);
            mapping.insert(b, va);
        }
    }
    Some(mapping)
}

/// Character id lookup: exact normalized name, else a unique prefix match —
/// saves/replays store 3-letter codes ("Cla") while start.gg names are full
/// ("Clairen"). Newer stations send full names; this keeps older ones working.
pub fn char_id_for(char_map: &HashMap<String, i64>, name: &Value) -> Option<i64> {
    let n = norm(name);
    if n.is_empty() {
        return None;
    }
    if let Some(id) = char_map.get(&n) {
        return Some(*id);
    }
    let mut hits: Vec<&String> = char_map.keys().filter(|k| k.starts_with(&n)).collect();
    // "Ran" prefixes both Ranno and Random; the save spells Random out in full
    // (matched exactly above), so a code never means it.
    if hits.len() > 1 {
        hits.retain(|k| k.as_str() != "random");
    }
    if hits.len() == 1 {
        Some(char_map[hits[0]])
    } else {
        None
    }
}

/// Per-game start.gg gameData: [{gameNum, winnerId?, selections?}].
pub fn game_data_from_games(
    games: &Value,
    slot_to_entrant: &HashMap<i64, String>,
    char_map: &HashMap<String, i64>,
) -> Value {
    let empty: Vec<Value> = Vec::new();
    let games = games.as_array().unwrap_or(&empty);
    let mut out: Vec<Value> = Vec::new();
    for g in games {
        let winner_id = g
            .get("winnerSlot")
            .filter(|v| !v.is_null())
            .and_then(|v| v.as_i64())
            .and_then(|s| slot_to_entrant.get(&s));
        let mut selections: Vec<Value> = Vec::new();
        for c in g.get("chars").and_then(|v| v.as_array()).unwrap_or(&empty) {
            let eid = c
                .get("slot")
                .and_then(|v| v.as_i64())
                .and_then(|s| slot_to_entrant.get(&s));
            let cid = char_id_for(char_map, c.get("character").unwrap_or(&Value::Null));
            if let (Some(eid), Some(cid)) = (eid, cid) {
                if !eid.is_empty() {
                    selections.push(json!({ "entrantId": eid, "characterId": cid }));
                }
            }
        }
        let mut game = serde_json::Map::new();
        game.insert(
            "gameNum".into(),
            g.get("gameNum").cloned().unwrap_or(Value::Null),
        );
        if let Some(w) = winner_id {
            if !w.is_empty() {
                game.insert("winnerId".into(), json!(w));
            }
        }
        if !selections.is_empty() {
            game.insert("selections".into(), Value::Array(selections));
        }
        out.push(Value::Object(game));
    }
    Value::Array(out)
}

/// True once start.gg's own copy of a set's games matches what was just
/// pushed via `update_live`. The mutation only echoes `{id state}` back,
/// never the game data itself, so this is the only way to tell a push
/// actually landed from merely "the HTTP call didn't error".
///
/// `pushed` is `game_data_from_games`'s own output (what was sent); `remote`
/// is whatever `StartggApi::set_games` read back: start.gg's raw `games`
/// field, `Null`/empty if the set has no games recorded yet, otherwise
/// `[{orderNum, winnerId, selections:[{entrant:{id}, character:{id}}]}]`.
/// IDs are compared as strings on both sides: what gets pushed uses JSON
/// strings, start.gg returns JSON numbers for the same ids, and the two
/// representations of one id still have to count as equal.
pub fn live_push_confirmed(pushed: &Value, remote: &Value) -> bool {
    fn id_str(v: &Value) -> String {
        v.to_string().trim_matches('"').to_string()
    }

    // (game number, winner id, sorted (entrantId, characterId) pairs) -- one
    // canonical, order-independent shape both sides get reduced to.
    type CanonGame = (i64, Option<String>, Vec<(String, String)>);

    fn canonicalize(
        games: &[Value],
        game_num_key: &str,
        selections_of: impl Fn(&Value) -> Vec<(String, String)>,
    ) -> Vec<CanonGame> {
        let mut out: Vec<_> = games
            .iter()
            .map(|g| {
                let num = g.get(game_num_key).and_then(|v| v.as_i64()).unwrap_or(0);
                let winner = g.get("winnerId").filter(|v| !v.is_null()).map(id_str);
                let mut sel = selections_of(g);
                sel.sort();
                (num, winner, sel)
            })
            .collect();
        out.sort_by_key(|(n, _, _)| *n);
        out
    }

    let empty: Vec<Value> = Vec::new();
    let pushed_games = pushed.as_array().unwrap_or(&empty);
    let remote_games = remote.as_array().unwrap_or(&empty);

    // Nothing was pushed, or start.gg has nothing yet: not a mismatch,
    // just nothing to confirm.
    if pushed_games.is_empty() || remote_games.is_empty() {
        return false;
    }

    let a = canonicalize(pushed_games, "gameNum", |g| {
        g.get("selections")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty)
            .iter()
            .map(|s| {
                (
                    s.get("entrantId").map(id_str).unwrap_or_default(),
                    s.get("characterId").map(id_str).unwrap_or_default(),
                )
            })
            .collect()
    });
    let b = canonicalize(remote_games, "orderNum", |g| {
        g.get("selections")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty)
            .iter()
            .map(|s| {
                (
                    s.get("entrant")
                        .and_then(|e| e.get("id"))
                        .map(id_str)
                        .unwrap_or_default(),
                    s.get("character")
                        .and_then(|c| c.get("id"))
                        .map(id_str)
                        .unwrap_or_default(),
                )
            })
            .collect()
    });

    a == b
}

// ---------------------------------------------------------------------------
// Tests: built around the real 2-0 set that exposed the live-score bug
// (JUGZ!/Orcane 2-0 KIM/Galvan, entrants jugeeya + Kimchi).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The real set, as the station writes it.
    fn real_set() -> Value {
        json!({
            "setId": "20260724_075508", "complete": true, "matchCount": 2,
            "winnerSlot": 0, "winnerName": "JUGZ!", "winnerCharacter": "Orc",
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
        build_tag_map(Some(&json!({
            "JUGZ!": "jugeeya", "KIM": "Kimchi", "BRUJITA": "Brujita"
        })))
    }

    /// start.gg's character list is full names; the save may send codes or full names.
    fn chars() -> HashMap<String, i64> {
        [
            ("orcane", 41),
            ("galvan", 42),
            ("clairen", 43),
            ("ranno", 44),
            ("random", 99),
            ("fleet", 45),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    fn live_map() -> HashMap<i64, String> {
        HashMap::from([(0, "24186345".to_string()), (1, "24186347".to_string())])
    }

    // ---- basics -------------------------------------------------------------

    #[test]
    fn norm_strips_punctuation() {
        assert_eq!(norm(&json!("JUGZ!")), "jugz", "norm strips punctuation");
    }

    #[test]
    fn entrant_names_splits_a_sponsor_prefix() {
        assert_eq!(
            entrant_names(&json!({"name": "TEAM | jugeeya"})),
            vec!["TEAM | jugeeya", "jugeeya"],
            "entrant_names splits a sponsor prefix"
        );
    }

    #[test]
    fn build_tag_map_normalizes_its_keys() {
        assert_eq!(
            tags().get("jugz"),
            Some(&"jugeeya".to_string()),
            "build_tag_map normalizes its keys"
        );
    }

    #[test]
    fn player_aliases_resolves_a_save_tag_through_players_json() {
        assert_eq!(
            player_aliases(Some(&json!({"name": "JUGZ!"})), Some(&tags())),
            vec!["jugeeya", "JUGZ!"],
            "player_aliases resolves a save tag through players.json"
        );
    }

    // ---- winner match ---------------------------------------------------------

    #[test]
    fn winner_jugz_maps_to_jugeeya_with_high_confidence() {
        let w = match_winner(&real_set(), Some(&entrants()), Some(&tags()));
        assert!(
            w.candidate_winner_entrant_id == Some(json!(24186345)) && w.confidence == "high",
            "winner JUGZ! -> jugeeya with high confidence"
        );
    }

    #[test]
    fn without_players_json_the_loser_kim_still_gives_jugeeya_by_elimination() {
        // "jugz" doesn't resemble "jugeeya" enough for any direct tier, but
        // the loser "KIM" partially matches "Kimchi" (and not jugeeya), so
        // elimination names jugeeya — at "low", since the loser match was a
        // substring guess rather than an exact alias.
        let w = match_winner(&real_set(), Some(&entrants()), None);
        assert_eq!(w.candidate_winner_entrant_id, Some(json!(24186345)));
        assert_eq!(w.confidence, "low");
    }

    // ---- per-game derivation (the 2-0 regression) -------------------------------

    #[test]
    fn two_games_derived() {
        let games = derive_games(&real_set());
        assert_eq!(games.as_array().unwrap().len(), 2, "two games derived");
    }

    #[test]
    fn both_games_won_by_slot_0() {
        let games = derive_games(&real_set());
        let winner_slots: Vec<Value> = games
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["winnerSlot"].clone())
            .collect();
        assert_eq!(
            winner_slots,
            vec![json!(0), json!(0)],
            "both games won by slot 0"
        );
    }

    #[test]
    fn game_numbers_preserved() {
        let games = derive_games(&real_set());
        assert_eq!(games[1]["gameNum"], json!(2), "game numbers preserved");
    }

    // ---- slot -> entrant mapping ------------------------------------------------

    #[test]
    fn live_mapping_by_tag() {
        let rec = json!({"entrants": entrants(), "set": summarize_set(&real_set())});
        let smap = map_slots_to_entrants(&rec, None, Some(&tags()));
        assert_eq!(
            smap,
            Some(live_map()),
            "live mapping by tag: slot0=jugeeya slot1=Kimchi  [{smap:?}]"
        );
    }

    #[test]
    fn swap_inverts_the_mapping() {
        let mut rec = json!({"entrants": entrants(), "set": summarize_set(&real_set())});
        rec["swap"] = json!(true);
        assert_eq!(
            map_slots_to_entrants(&rec, None, Some(&tags())),
            Some(HashMap::from([
                (0, "24186347".to_string()),
                (1, "24186345".to_string())
            ])),
            "swap inverts the mapping"
        );
    }

    #[test]
    fn finalizing_anchors_the_winners_slot() {
        let rec = json!({"entrants": entrants(), "set": summarize_set(&real_set())});
        let fin = map_slots_to_entrants(&rec, Some(&json!(24186345)), Some(&tags()));
        assert_eq!(
            fin,
            Some(live_map()),
            "finalizing anchors the winner's slot"
        );
    }

    #[test]
    fn unknown_names_still_pair_by_order() {
        // unknown tags still map (by order) rather than refusing
        let mut set = real_set();
        set["players"] = json!([
            {"slot": 0, "name": "ZZZ", "character": "Orc", "wins": 2},
            {"slot": 1, "name": "YYY", "character": "Gal", "wins": 0},
        ]);
        let rec_unknown = json!({"entrants": entrants(), "set": summarize_set(&set)});
        assert!(
            map_slots_to_entrants(&rec_unknown, None, Some(&tags())).is_some(),
            "unknown names still pair by order (never refuse a live score)"
        );
    }

    // ---- character ids -----------------------------------------------------------

    #[test]
    fn three_letter_code_orc_maps_to_orcane_by_unique_prefix() {
        assert_eq!(
            char_id_for(&chars(), &json!("Orc")),
            Some(41),
            "3-letter code 'Orc' -> Orcane by unique prefix"
        );
    }

    #[test]
    fn full_name_orcane_maps_exactly() {
        assert_eq!(
            char_id_for(&chars(), &json!("Orcane")),
            Some(41),
            "full name 'Orcane' -> exact"
        );
    }

    #[test]
    fn ran_prefers_ranno_over_random() {
        assert_eq!(
            char_id_for(&chars(), &json!("Ran")),
            Some(44),
            "'Ran' prefers Ranno over Random"
        );
    }

    #[test]
    fn random_exact_matches_random() {
        assert_eq!(
            char_id_for(&chars(), &json!("Random")),
            Some(99),
            "'Random' exact-matches Random"
        );
    }

    #[test]
    fn unknown_character_is_none() {
        assert_eq!(
            char_id_for(&chars(), &json!("Nope")),
            None,
            "unknown character -> None"
        );
    }

    // ---- start.gg gameData (what actually gets pushed) ---------------------------

    #[test]
    fn game_data_covers_both_games() {
        let gd = game_data_from_games(&derive_games(&real_set()), &live_map(), &chars());
        assert_eq!(
            gd.as_array().unwrap().len(),
            2,
            "gameData covers both games"
        );
    }

    #[test]
    fn both_games_carry_a_winner_id() {
        let gd = game_data_from_games(&derive_games(&real_set()), &live_map(), &chars());
        assert!(
            gd.as_array()
                .unwrap()
                .iter()
                .all(|g| g.get("winnerId") == Some(&json!("24186345"))),
            "BOTH games carry a winnerId (the bug that froze start.gg at 1-0)"
        );
    }

    #[test]
    fn character_selections_resolved_per_entrant() {
        let gd = game_data_from_games(&derive_games(&real_set()), &live_map(), &chars());
        assert_eq!(
            gd[0]["selections"],
            json!([
                {"entrantId": "24186345", "characterId": 41},
                {"entrantId": "24186347", "characterId": 42},
            ]),
            "character selections resolved per entrant"
        );
    }

    /// start.gg's read side returns ids as JSON numbers where the pushed
    /// gameData used strings (confirmed against the live schema, not
    /// assumed) -- the exact same push, read back with that representation
    /// swapped, must still count as confirmed.
    #[test]
    fn live_push_confirmed_true_for_the_same_push_with_numeric_ids() {
        let gd = game_data_from_games(&derive_games(&real_set()), &live_map(), &chars());
        let remote = json!([
            {"orderNum": 1, "winnerId": 24186345, "selections": [
                {"entrant": {"id": 24186345}, "character": {"id": 41}},
                {"entrant": {"id": 24186347}, "character": {"id": 42}},
            ]},
            {"orderNum": 2, "winnerId": 24186345, "selections": [
                {"entrant": {"id": 24186345}, "character": {"id": 41}},
                {"entrant": {"id": 24186347}, "character": {"id": 42}},
            ]},
        ]);
        assert!(live_push_confirmed(&gd, &remote));
    }

    #[test]
    fn live_push_confirmed_false_before_anything_is_pushed() {
        assert!(!live_push_confirmed(&json!([]), &json!(null)));
    }

    /// start.gg hasn't recorded any games for the set yet -- not an error,
    /// just "not confirmed on this attempt"; the next live push tries again.
    #[test]
    fn live_push_confirmed_false_when_start_gg_has_nothing_yet() {
        let gd = game_data_from_games(&derive_games(&real_set()), &live_map(), &chars());
        assert!(!live_push_confirmed(&gd, &Value::Null));
        assert!(!live_push_confirmed(&gd, &json!([])));
    }

    /// A genuine divergence (wrong winner) must not read as confirmed --
    /// this is what would have papered over a bad push silently.
    #[test]
    fn live_push_confirmed_false_on_a_real_mismatch() {
        let gd = game_data_from_games(&derive_games(&real_set()), &live_map(), &chars());
        let remote_wrong_winner = json!([
            {"orderNum": 1, "winnerId": 24186347, "selections": [
                {"entrant": {"id": 24186345}, "character": {"id": 41}},
                {"entrant": {"id": 24186347}, "character": {"id": 42}},
            ]},
            {"orderNum": 2, "winnerId": 24186345, "selections": [
                {"entrant": {"id": 24186345}, "character": {"id": 41}},
                {"entrant": {"id": 24186347}, "character": {"id": 42}},
            ]},
        ]);
        assert!(!live_push_confirmed(&gd, &remote_wrong_winner));
    }
}

/// Case study: every bracket set station 1 recorded at The Hangout 4.1
/// (tournament/the-hangout-4-1/event/rivals-of-aether-ii-singles, 2026-08-08)
/// — real in-game tags, real entrants, real winners, and the tag-database
/// snapshot that was cached on the machine that night. Before the
/// symbol-tag/elimination/abbreviation/corroboration work, 8 of these 13
/// sets matched NO candidate winner (and only 3 could auto-report); after
/// it, all 13 name the right entrant at auto-report confidence.
#[cfg(test)]
mod hangout41 {
    use super::*;
    use serde_json::json;

    /// The tag database as cached on station 1 that night — plus "***",
    /// which WAS published (threeleggeddog's tag) but was being dropped by
    /// the old key normalisation; `norm_key` keeps it now.
    fn tagdb() -> HashMap<String, String> {
        [
            ("alvrg", "JuanSolo"),
            ("bub", "Kimchi"),
            ("jugz", "jugeeya"),
            ("45to", "Ahntye"),
            ("dalek", "Dalek Ditto"),
            ("altn", "AltonP"),
            ("joule", "Joule Thief"),
            ("kim", "Kimchi"),
            ("***", "threeleggeddog"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn set_1v1(winner: (&str, &str), loser: (&str, &str)) -> Value {
        json!({
            "complete": true, "winnerName": winner.0,
            "players": [
                {"slot": 0, "name": winner.0, "character": winner.1, "wins": 3},
                {"slot": 1, "name": loser.0, "character": loser.1, "wins": 1},
            ],
        })
    }

    fn ents(a: (i64, &str), b: (i64, &str)) -> Value {
        json!([{"id": a.0, "name": a.1}, {"id": b.0, "name": b.1}])
    }

    /// (start.gg set, winner tag+char, loser tag+char, entrants,
    ///  expected winner entrant id, expected confidence)
    #[allow(clippy::type_complexity)]
    fn the_night() -> Vec<(
        &'static str,
        (&'static str, &'static str),
        (&'static str, &'static str),
        Value,
        i64,
        &'static str,
    )> {
        let orbital = (24311105, "Orbital");
        let ahntye = (24328362, "Runback | Ahntye");
        let fizzy = (24260111, "FizzyBrax | Potatoes");
        let juansolo = (24260328, "POA | JuanSolo");
        let mfg = (24310923, "1mfg | threeleggeddog");
        let sama = (24328678, "Samasicus");
        let kimchi = (24186347, "Kimchi");
        let joule = (24300357, "Joule Thief");
        let jugeeya = (24186345, "jugeeya");
        let coopr = (24339847, "IKKI | coopR");
        vec![
            // WR2: winner's "ORB" is only a substring guess, but the loser's
            // 45TO is Ahntye's published tag — elimination, exact.
            (
                "106270105",
                ("ORB", "Etalus"),
                ("45TO", "Ranno"),
                ents(orbital, ahntye),
                orbital.0,
                "high",
            ),
            // WQF: SHLNG published nothing; ALVRG is JuanSolo's tag.
            (
                "106270106",
                ("SHLNG", "Loxodont"),
                ("ALVRG", "Loxodont"),
                ents(fizzy, juansolo),
                fizzy.0,
                "high",
            ),
            // WQF: the symbols-only tag, straight from the tag database.
            (
                "106270108",
                ("***", "Kragg"),
                ("SMSC", "Wrastor"),
                ents(mfg, sama),
                mfg.0,
                "high",
            ),
            // WQF: BUB:) normalises to "bub" -> Kimchi.
            (
                "106270107",
                ("BUB:)", "Zetterburn"),
                ("JOULE", "Fleet"),
                ents(kimchi, joule),
                kimchi.0,
                "high",
            ),
            (
                "106270109",
                ("JUGZ!", "Fleet"),
                ("ORB", "Etalus"),
                ents(jugeeya, orbital),
                jugeeya.0,
                "high",
            ),
            // LR3: neither tag is certain alone, but the two partial reads
            // corroborate — orb ⊂ Orbital, c ⊂ coopR, nothing crossed.
            (
                "106270169",
                ("ORB", "Etalus"),
                ("C", "Absa"),
                ents(orbital, coopr),
                orbital.0,
                "high",
            ),
            (
                "106270110",
                ("SHLNG", "Loxodont"),
                ("BUB:)", "Zetterburn"),
                ents(fizzy, kimchi),
                fizzy.0,
                "high",
            ),
            (
                "106270111",
                ("JUGZ!", "Fleet"),
                ("***", "Kragg"),
                ents(mfg, jugeeya),
                jugeeya.0,
                "high",
            ),
            (
                "106270173",
                ("***", "Kragg"),
                ("JOULE", "Fleet"),
                ents(mfg, joule),
                mfg.0,
                "high",
            ),
            (
                "106270112",
                ("SHLNG", "Loxodont"),
                ("JUGZ!", "Fleet"),
                ents(fizzy, jugeeya),
                fizzy.0,
                "high",
            ),
            (
                "106270175",
                ("***", "Kragg"),
                ("BUB:)", "Zetterburn"),
                ents(mfg, kimchi),
                mfg.0,
                "high",
            ),
            (
                "106270176",
                ("***", "Kragg"),
                ("JUGZ!", "Fleet"),
                ents(jugeeya, mfg),
                mfg.0,
                "high",
            ),
            // Grand Final: neither tag matches directly — SHLNG published
            // nothing and "***" needs the symbol fix — but with it, the
            // loser is exactly threeleggeddog, so elimination names Fizzy.
            (
                "106270113",
                ("SHLNG", "Loxodont"),
                ("***", "Kragg"),
                ents(fizzy, mfg),
                fizzy.0,
                "high",
            ),
        ]
    }

    #[test]
    fn every_bracket_set_names_the_right_winner() {
        let tags = tagdb();
        for (set_id, winner, loser, entrants, want_id, want_conf) in the_night() {
            let st = set_1v1(winner, loser);
            let w = match_winner(&st, Some(&entrants), Some(&tags));
            assert_eq!(
                w.candidate_winner_entrant_id,
                Some(json!(want_id)),
                "set {set_id}: {} vs {} should name entrant {want_id}, got {:?} ({})",
                winner.0,
                loser.0,
                w.candidate_winner_entrant_id,
                w.confidence,
            );
            assert_eq!(
                w.confidence, want_conf,
                "set {set_id}: {} vs {} expected {want_conf}",
                winner.0, loser.0,
            );
        }
    }

    #[test]
    fn the_whole_night_would_auto_report() {
        // Before this work: 3 of 13. The last holdout was ORB vs C, carried
        // over the line by mutual corroboration of the two partial reads.
        let tags = tagdb();
        let high = the_night()
            .into_iter()
            .filter(|(_, w, l, entrants, _, _)| {
                match_winner(&set_1v1(*w, *l), Some(entrants), Some(&tags)).confidence == "high"
            })
            .count();
        assert_eq!(high, 13);
    }

    #[test]
    fn smsc_reads_as_an_abbreviation_of_samasicus() {
        // Samasicus never won on this station that night, so the
        // abbreviation tier wasn't load-bearing above — but their tag is the
        // canonical example of one. With an unknown opponent (no elimination
        // available), the abbreviation alone should name them at "low".
        let st = set_1v1(("SMSC", "Wrastor"), ("XYZQ", "Kragg"));
        let entrants = ents((24328678, "Samasicus"), (24311105, "Orbital"));
        let w = match_winner(&st, Some(&entrants), Some(&tagdb()));
        assert_eq!(w.candidate_winner_entrant_id, Some(json!(24328678)));
        assert_eq!(w.confidence, "low");
    }

    #[test]
    fn symbol_tags_survive_normalisation() {
        assert_eq!(norm_key("***"), "***");
        assert_eq!(norm_key("BUB:)"), "bub", "mixed tags still strip to alnum");
        assert_eq!(norm_key("JUGZ!"), "jugz");
    }

    #[test]
    fn symbol_tags_map_slots_for_live_scores() {
        // Live pushes need slot -> entrant too; "***" must map exactly, not
        // fall to pair-by-order.
        let st = set_1v1(("***", "Kragg"), ("SHLNG", "Loxodont"));
        let rec = json!({
            "entrants": ents((24310923, "1mfg | threeleggeddog"), (24260111, "FizzyBrax | Potatoes")),
            "set": summarize_set(&st),
        });
        let smap = map_slots_to_entrants(&rec, None, Some(&tagdb()));
        assert_eq!(
            smap,
            Some(HashMap::from([
                (0, "24310923".to_string()),
                (1, "24260111".to_string()),
            ]))
        );
    }

    #[test]
    fn a_partial_guess_that_fits_both_entrants_names_nobody() {
        // "AL" substring-hits both ALTN-ish entrants; a guess that points
        // two ways points nowhere (the old code took whichever came first).
        let st = set_1v1(("AL", "Kragg"), ("XYZQ", "Fleet"));
        let entrants = ents((1, "Alton"), (2, "Alvara"));
        let w = match_winner(&st, Some(&entrants), None);
        assert_eq!(w.confidence, "none");
    }

    #[test]
    fn crossed_partial_reads_do_not_corroborate() {
        // Winner's guess fits Orbital — but so does the LOSER's. Two reads
        // pointing at the same entrant contradict rather than corroborate;
        // the winner's own guess stands, at "low" only.
        let st = set_1v1(("ORB", "Etalus"), ("BITAL", "Absa"));
        let entrants = ents((24311105, "Orbital"), (24339847, "IKKI | coopR"));
        let w = match_winner(&st, Some(&entrants), Some(&tagdb()));
        assert_eq!(w.candidate_winner_entrant_id, Some(json!(24311105)));
        assert_eq!(w.confidence, "low");
    }
}
