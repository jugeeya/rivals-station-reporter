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

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

/// start.gg Set.state:
///   1 = created (seeded, not called)   2 = ongoing — the TO pressed "Start Match"
///   3 = completed                      6 = called to a station, still not started
/// Only 2 means the match is genuinely underway. Until then a station's games are
/// warmups/friendlies at that setup, so they must not be bound to the bracket set
/// or pushed to it — and we never start a match ourselves.
pub const STARTGG_STATE_ONGOING: i64 = 2;

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
            let nk = norm_str(k);
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
        let key = norm(p.get("name").unwrap_or(&Value::Null));
        if let Some(via) = tm.get(&key) {
            if !via.is_empty() {
                out.push(via.clone());
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

/// Best-effort winner match -> candidate entrant id + confidence.
///
/// In-game names rarely equal start.gg tags, so an unsure result is expected —
/// the operator confirms before anything is finalized.
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

    let mut winner_player: Option<&Value> = None;
    if let Some(players) = st.get("players").and_then(|v| v.as_array()) {
        for p in players {
            if p.get("name") == Some(winner_name) {
                winner_player = Some(p);
                break;
            }
        }
    }
    let fallback = json!({ "name": winner_name });
    let aliases: Vec<String> = player_aliases(Some(winner_player.unwrap_or(&fallback)), tag_map)
        .iter()
        .map(|a| norm_str(a))
        .filter(|a| !a.is_empty())
        .collect();
    if aliases.is_empty() {
        return none;
    }

    let mut partial: Option<&Value> = None;
    for e in entrants_list {
        for n in entrant_names(e) {
            let nn = norm_str(&n);
            if nn.is_empty() {
                continue;
            }
            if aliases.iter().any(|a| a == &nn) {
                return WinnerMatch {
                    candidate_winner_entrant_id: e.get("id").filter(|v| !v.is_null()).cloned(),
                    confidence: "high",
                };
            }
            if partial.is_none()
                && aliases
                    .iter()
                    .any(|a| nn.contains(a.as_str()) || a.contains(&nn))
            {
                partial = Some(e);
            }
        }
    }
    if let Some(e) = partial {
        return WinnerMatch {
            candidate_winner_entrant_id: e.get("id").filter(|v| !v.is_null()).cloned(),
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
            let aliases: Vec<String> = player_aliases(Some(p), tag_map)
                .iter()
                .map(|a| norm_str(a))
                .filter(|a| !a.is_empty())
                .collect();
            if aliases.is_empty() {
                continue;
            }
            let mut hit: Option<&Value> = None;
            for e in entrants {
                if used.contains(&id_key(e)) {
                    continue;
                }
                for n in entrant_names(e) {
                    let nn = norm_str(&n);
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
    fn without_players_json_jugz_vs_jugeeya_is_honestly_none() {
        let w = match_winner(&real_set(), Some(&entrants()), None);
        assert_eq!(
            w.confidence, "none",
            "without players.json, JUGZ! vs jugeeya is honestly 'none'"
        );
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
