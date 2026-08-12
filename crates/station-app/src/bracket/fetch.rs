//! Whole-bracket read from start.gg — the UNAUTHENTICATED website endpoint,
//! same one `vodsplit::sets` and `tags::bracket` already use. No personal
//! token needed, so the bracket is viewable on a station install too; only
//! the mutations (assign / start / report) go through the operator's token.
//!
//! This is deliberately NOT `station-core`'s `AVAILABLE_SETS_QUERY`. That one
//! filters to `state:[1,2,6]` because the Current Sets panel only ever wants
//! what's actionable right now; a bracket needs every set, completed and
//! not-yet-seeded alike, or the tree has holes where the finished rounds were.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::json;

const ENDPOINT: &str = "https://www.start.gg/api/-/gql";
// The website API 403s obvious library user agents; same string as
// station-core's `startgg_web` and the VOD splitter's fetcher.
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Mobile Safari/537.36";
const PER_PAGE: u32 = 60;
/// 60 sets/page — 20 pages is a 1200-set event, far past anything this runs
/// against, and bounds a pathological response instead of paging forever.
const MAX_PAGES: u32 = 20;
const TIMEOUT_SECS: u64 = 30;

/// `round`, `identifier` and `winnerId` are what make this a bracket rather
/// than a list: `round` is signed (positive = winners, negative = losers) and
/// its magnitude is the column, `identifier` orders sets within a round, and
/// `winnerId` marks the advancing entrant. `slots.standing.stats.score.value`
/// is the per-entrant game count — negative when an entrant was DQ'd, which
/// the UI renders as "DQ" rather than a score.
const QUERY: &str = r#"
query Bracket($slug: String!, $page: Int!, $perPage: Int!) {
  event(slug: $slug) {
    id
    name
    tournament {
      name
      stations(perPage: 32) { nodes { number } }
      streams { streamName }
    }
    sets(page: $page, perPage: $perPage, sortType: STANDARD) {
      pageInfo { totalPages }
      nodes {
        id
        state
        round
        identifier
        fullRoundText
        winnerId
        totalGames
        startedAt
        completedAt
        station { number }
        stream { streamName }
        phaseGroup {
          id
          displayIdentifier
          bracketType
          phase { id name phaseOrder }
        }
        games { selections { entrant { id } character { name } } }
        slots {
          slotIndex
          prereqId
          prereqType
          entrant { id name }
          standing { stats { score { value } } }
        }
      }
    }
  }
}
"#;

/// One side of a set. `entrant` is `None` while the slot is still waiting on
/// an earlier round — the bracket shows those as empty seats rather than
/// dropping the set the way `parse_available_sets` has to.
#[derive(Debug, Clone, Default)]
pub struct Slot {
    pub entrant_id: Option<String>,
    pub name: Option<String>,
    /// Games won. Negative means DQ (start.gg encodes it as -1).
    pub score: Option<i64>,
    /// The character this entrant played, from the set's own game data. Only
    /// present once games have been reported with selections.
    pub character: Option<String>,
    /// The set that feeds this seat, when one does. `None` for a seat filled
    /// straight from seeding, and also for a feeder start.gg didn't return —
    /// a bracket with byes references sets that never come back in the list,
    /// so a dangling link has to read the same as no link at all.
    pub prereq_set_id: Option<String>,
}

impl Slot {
    pub fn is_dq(&self) -> bool {
        self.score.is_some_and(|s| s < 0)
    }

    /// What to draw in the score column: nothing until the set has a result.
    pub fn score_text(&self) -> String {
        match self.score {
            Some(s) if s < 0 => "DQ".to_string(),
            Some(s) => s.to_string(),
            None => String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BracketSet {
    pub id: String,
    /// start.gg reports a not-yet-started bracket's sets with placeholder ids
    /// ("preview_3396320_1_0"). They can't be assigned, started or reported —
    /// the UI disables the actions rather than letting start.gg answer with
    /// "An unknown error has occurred".
    pub preview: bool,
    /// Raw `Set.state`: 1 created, 2 ongoing, 3 completed, 6 called.
    pub state: i64,
    /// Signed round. Positive = winners side, negative = losers side; the
    /// magnitude is the column within that side.
    pub round: i64,
    /// Set label within the round ("A", "Z", "AA"…). Orders sets top-to-bottom.
    pub identifier: String,
    pub full_round_text: String,
    pub winner_id: Option<String>,
    pub total_games: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub station: Option<i64>,
    pub stream: Option<String>,
    pub phase_group_id: String,
    pub phase_group_label: String,
    pub phase_name: String,
    pub phase_order: i64,
    pub bracket_type: String,
    /// Always exactly two, padded with empty slots if start.gg sent fewer.
    pub slots: [Slot; 2],
}

impl BracketSet {
    pub fn is_complete(&self) -> bool {
        self.state == STATE_COMPLETED
    }

    pub fn is_ongoing(&self) -> bool {
        self.state == STATE_ONGOING
    }

    /// Called to a setup but not started — the state a set sits in between a
    /// TO assigning it and the players actually beginning.
    pub fn is_called(&self) -> bool {
        self.state == STATE_CALLED
    }

    /// Both seats filled — the set can actually be played (and so assigned,
    /// started, or reported).
    pub fn is_ready(&self) -> bool {
        self.slots.iter().all(|s| s.entrant_id.is_some())
    }

    /// Both seats filled and nobody has called it yet — the set a TO should
    /// be putting on the next free setup. A preview set counts: starting one
    /// is what brings the bracket to life.
    pub fn is_startable(&self) -> bool {
        self.is_ready() && !self.is_complete() && !self.is_ongoing() && !self.is_called()
    }

    pub fn winner_slot(&self) -> Option<usize> {
        let w = self.winner_id.as_deref()?;
        self.slots
            .iter()
            .position(|s| s.entrant_id.as_deref() == Some(w))
    }
}

pub const STATE_CREATED: i64 = 1;
pub const STATE_ONGOING: i64 = 2;
pub const STATE_COMPLETED: i64 = 3;
pub const STATE_CALLED: i64 = 6;

#[derive(Debug, Clone, Default)]
pub struct Bracket {
    pub event_name: String,
    pub tournament_name: String,
    pub sets: Vec<BracketSet>,
    /// Station numbers the TOURNAMENT has configured, for the action bar's
    /// picker. Tournament-level, not `Event.stations` — that one only returns
    /// stations already assigned to a set in this event, so a picker built
    /// from it silently offers fewer setups than really exist (the same trap
    /// `startgg::AVAILABLE_SETS_QUERY` documents at length).
    pub stations: Vec<i64>,
    /// Stream names the tournament has configured, same source and reason.
    pub streams: Vec<String>,
}

// ---- raw response shapes ---------------------------------------------------

#[derive(Deserialize)]
struct Resp {
    data: Option<RespData>,
    #[serde(default)]
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Deserialize)]
struct RespData {
    event: Option<RawEvent>,
}

#[derive(Deserialize)]
struct RawEvent {
    name: Option<String>,
    tournament: Option<RawTournament>,
    sets: Option<RawSets>,
}

#[derive(Deserialize)]
struct RawTournament {
    name: Option<String>,
    #[serde(default)]
    stations: Option<RawStations>,
    #[serde(default)]
    streams: Option<Vec<RawTournamentStream>>,
}

#[derive(Deserialize)]
struct RawTournamentStream {
    #[serde(rename = "streamName")]
    stream_name: Option<String>,
}

#[derive(Deserialize)]
struct RawStations {
    #[serde(default)]
    nodes: Option<Vec<RawStationNode>>,
}

#[derive(Deserialize)]
struct RawStationNode {
    number: Option<i64>,
}

#[derive(Deserialize)]
struct RawNamed {
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawSets {
    #[serde(rename = "pageInfo")]
    page_info: Option<RawPageInfo>,
    nodes: Option<Vec<RawSet>>,
}

#[derive(Deserialize)]
struct RawPageInfo {
    #[serde(rename = "totalPages")]
    total_pages: Option<u32>,
}

#[derive(Deserialize)]
struct RawSet {
    id: Option<serde_json::Value>,
    state: Option<i64>,
    round: Option<i64>,
    identifier: Option<String>,
    #[serde(rename = "fullRoundText")]
    full_round_text: Option<String>,
    #[serde(rename = "winnerId")]
    winner_id: Option<serde_json::Value>,
    #[serde(rename = "totalGames")]
    total_games: Option<i64>,
    #[serde(rename = "startedAt")]
    started_at: Option<i64>,
    #[serde(rename = "completedAt")]
    completed_at: Option<i64>,
    station: Option<RawStation>,
    stream: Option<RawStream>,
    #[serde(rename = "phaseGroup")]
    phase_group: Option<RawPhaseGroup>,
    games: Option<Vec<RawGame>>,
    slots: Option<Vec<RawSlot>>,
}

#[derive(Deserialize)]
struct RawStation {
    number: Option<i64>,
}

#[derive(Deserialize)]
struct RawStream {
    #[serde(rename = "streamName")]
    stream_name: Option<String>,
}

#[derive(Deserialize)]
struct RawPhaseGroup {
    id: Option<serde_json::Value>,
    #[serde(rename = "displayIdentifier")]
    display_identifier: Option<String>,
    #[serde(rename = "bracketType")]
    bracket_type: Option<String>,
    phase: Option<RawPhase>,
}

#[derive(Deserialize)]
struct RawPhase {
    name: Option<String>,
    #[serde(rename = "phaseOrder")]
    phase_order: Option<i64>,
}

#[derive(Deserialize)]
struct RawSlot {
    entrant: Option<RawEntrant>,
    standing: Option<RawStanding>,
    #[serde(rename = "prereqId")]
    prereq_id: Option<serde_json::Value>,
    #[serde(rename = "prereqType")]
    prereq_type: Option<String>,
}

#[derive(Deserialize)]
struct RawGame {
    selections: Option<Vec<RawSelection>>,
}

#[derive(Deserialize)]
struct RawSelection {
    entrant: Option<RawEntrant>,
    character: Option<RawNamed>,
}

#[derive(Deserialize)]
struct RawEntrant {
    id: Option<serde_json::Value>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawStanding {
    stats: Option<RawStats>,
}

#[derive(Deserialize)]
struct RawStats {
    score: Option<RawScore>,
}

#[derive(Deserialize)]
struct RawScore {
    value: Option<f64>,
}

/// GraphQL ids arrive as numbers for real rows and strings for placeholders;
/// stringify both so everything downstream compares like for like (the same
/// normalization `parse_available_sets` does with `py_str`).
fn id_of(v: Option<&serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// entrant id -> the character they played, from the first game that named
/// one. Later games can differ (counterpicks); the first is what the card
/// shows, the same "first selection wins" rule `vodsplit::sets` uses when it
/// builds clip titles.
fn characters_of(games: &Option<Vec<RawGame>>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for game in games.iter().flatten() {
        for sel in game.selections.iter().flatten() {
            let (Some(id), Some(name)) = (
                id_of(sel.entrant.as_ref().and_then(|e| e.id.as_ref())),
                sel.character.as_ref().and_then(|c| c.name.clone()),
            ) else {
                continue;
            };
            out.entry(id).or_insert(name);
        }
    }
    out
}

fn slot_of(raw: Option<&RawSlot>, characters: &HashMap<String, String>) -> Slot {
    let Some(raw) = raw else {
        return Slot::default();
    };
    let entrant_id = id_of(raw.entrant.as_ref().and_then(|e| e.id.as_ref()));
    Slot {
        character: entrant_id
            .as_deref()
            .and_then(|id| characters.get(id))
            .cloned(),
        // Only a "set" prereq names a feeding set; a "seed" one names an
        // entry seed and must not be followed as though it were a set.
        prereq_set_id: (raw.prereq_type.as_deref() == Some("set"))
            .then(|| id_of(raw.prereq_id.as_ref()))
            .flatten(),
        entrant_id,
        name: raw
            .entrant
            .as_ref()
            .and_then(|e| e.name.clone())
            .filter(|n| !n.is_empty()),
        // Scores come back as floats (start.gg's score field is generic
        // enough to carry non-integer values for other games); Rivals only
        // ever has whole games, and -1 for a DQ.
        score: raw
            .standing
            .as_ref()
            .and_then(|s| s.stats.as_ref())
            .and_then(|s| s.score.as_ref())
            .and_then(|s| s.value)
            .map(|v| v.round() as i64),
    }
}

fn convert(node: RawSet) -> Option<BracketSet> {
    let id = id_of(node.id.as_ref())?;
    let characters = characters_of(&node.games);
    let slots = node.slots.unwrap_or_default();
    let phase_group = node.phase_group;
    Some(BracketSet {
        preview: id.starts_with("preview"),
        state: node.state.unwrap_or(STATE_CREATED),
        round: node.round.unwrap_or(0),
        identifier: node.identifier.unwrap_or_default(),
        full_round_text: node.full_round_text.unwrap_or_default(),
        winner_id: id_of(node.winner_id.as_ref()),
        total_games: node.total_games,
        started_at: node.started_at,
        completed_at: node.completed_at,
        station: node.station.and_then(|s| s.number),
        stream: node
            .stream
            .and_then(|s| s.stream_name)
            .filter(|s| !s.is_empty()),
        phase_group_id: phase_group
            .as_ref()
            .and_then(|p| id_of(p.id.as_ref()))
            .unwrap_or_default(),
        phase_group_label: phase_group
            .as_ref()
            .and_then(|p| p.display_identifier.clone())
            .unwrap_or_default(),
        phase_name: phase_group
            .as_ref()
            .and_then(|p| p.phase.as_ref())
            .and_then(|p| p.name.clone())
            .unwrap_or_default(),
        phase_order: phase_group
            .as_ref()
            .and_then(|p| p.phase.as_ref())
            .and_then(|p| p.phase_order)
            .unwrap_or(0),
        bracket_type: phase_group
            .as_ref()
            .and_then(|p| p.bracket_type.clone())
            .unwrap_or_default(),
        slots: [
            slot_of(slots.first(), &characters),
            slot_of(slots.get(1), &characters),
        ],
        id,
    })
}

/// Fetch every set in an event, following pagination.
pub async fn fetch(slug: String) -> Result<Bracket, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let mut out = Bracket::default();
    let mut page = 1u32;
    let mut total_pages = 1u32;

    while page <= total_pages && page <= MAX_PAGES {
        let body = json!({
            "query": QUERY,
            "variables": { "slug": slug, "page": page, "perPage": PER_PAGE },
        });
        let resp = client
            .post(ENDPOINT)
            .header("client-version", "20")
            .header("User-Agent", USER_AGENT)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        let status = resp.status();
        let parsed: Resp = resp
            .json()
            .await
            .map_err(|e| format!("start.gg returned something unreadable (HTTP {status}): {e}"))?;

        if let Some(errors) = parsed.errors {
            if !errors.is_empty() {
                let joined: Vec<String> = errors.into_iter().map(|e| e.message).collect();
                return Err(joined.join("; "));
            }
        }
        if !status.is_success() {
            return Err(format!("start.gg returned HTTP {status}"));
        }

        // A bad slug comes back as `event: null`, not as an error — reading
        // that as an empty bracket would show "no sets" for what is really a
        // wrong event link (the same trap `available_sets` guards against).
        let event = parsed
            .data
            .and_then(|d| d.event)
            .ok_or_else(|| "start.gg has no event at that link — check it in Settings.")?;

        if page == 1 {
            out.event_name = event.name.clone().unwrap_or_default();
            if let Some(t) = &event.tournament {
                out.tournament_name = t.name.clone().unwrap_or_default();
                out.stations = t
                    .stations
                    .as_ref()
                    .and_then(|s| s.nodes.as_ref())
                    .map(|nodes| nodes.iter().filter_map(|n| n.number).collect())
                    .unwrap_or_default();
                out.stations.sort_unstable();
                out.streams = t
                    .streams
                    .as_ref()
                    .map(|list| {
                        list.iter()
                            .filter_map(|s| s.stream_name.clone())
                            .filter(|n| !n.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }

        let sets = event.sets.ok_or("Event has no bracket yet.")?;
        if page == 1 {
            total_pages = sets
                .page_info
                .as_ref()
                .and_then(|p| p.total_pages)
                .unwrap_or(1)
                .max(1);
        }
        out.sets.extend(
            sets.nodes
                .unwrap_or_default()
                .into_iter()
                .filter_map(convert),
        );
        page += 1;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Bracket {
        let parsed: Resp = serde_json::from_str(body).expect("fixture parses");
        let event = parsed.data.and_then(|d| d.event).expect("event present");
        let sets = event.sets.expect("sets present");
        Bracket {
            stations: Vec::new(),
            streams: Vec::new(),
            event_name: event.name.unwrap_or_default(),
            tournament_name: event.tournament.and_then(|t| t.name).unwrap_or_default(),
            sets: sets
                .nodes
                .unwrap_or_default()
                .into_iter()
                .filter_map(convert)
                .collect(),
        }
    }

    /// Shapes taken verbatim from a live response for The Hangout #4.
    const FIXTURE: &str = r#"{"data":{"event":{
      "id":1,"name":"Rivals of Aether II Singles","tournament":{"name":"The Hangout #4"},
      "sets":{"pageInfo":{"totalPages":1},"nodes":[
        {"id":106270113,"state":3,"round":6,"identifier":"T","fullRoundText":"Grand Final",
         "winnerId":24260111,"totalGames":5,"startedAt":1786246264,"completedAt":1786247392,
         "station":{"number":1},"stream":{"streamName":"socalrivals"},
         "phaseGroup":{"id":3396320,"displayIdentifier":"1","bracketType":"DOUBLE_ELIMINATION",
                       "phase":{"id":2351127,"name":"Bracket","phaseOrder":1}},
         "slots":[
           {"slotIndex":0,"entrant":{"id":24260111,"name":"FizzyBrax | Potatoes"},
            "standing":{"stats":{"score":{"value":3}}}},
           {"slotIndex":1,"entrant":{"id":24310923,"name":"1mfg | threeleggeddog"},
            "standing":{"stats":{"score":{"value":1}}}}]},
        {"id":"preview_3396320_1_0","state":1,"round":-10,"identifier":"AM",
         "fullRoundText":"Losers Final","winnerId":null,"totalGames":null,
         "startedAt":null,"completedAt":null,"station":null,"stream":null,
         "phaseGroup":{"id":3396320,"displayIdentifier":"1","bracketType":"DOUBLE_ELIMINATION",
                       "phase":{"id":2351127,"name":"Bracket","phaseOrder":1}},
         "slots":[{"slotIndex":0,"entrant":null,"standing":null},
                  {"slotIndex":1,"entrant":null,"standing":null}]},
        {"id":106270100,"state":3,"round":-4,"identifier":"V","fullRoundText":"Losers Round 1",
         "winnerId":24260111,"totalGames":3,"startedAt":null,"completedAt":null,
         "station":null,"stream":null,
         "phaseGroup":{"id":3396320,"displayIdentifier":"1","bracketType":"DOUBLE_ELIMINATION",
                       "phase":{"id":2351127,"name":"Bracket","phaseOrder":1}},
         "slots":[
           {"slotIndex":0,"entrant":{"id":24260111,"name":"Potatoes"},
            "standing":{"stats":{"score":{"value":2}}}},
           {"slotIndex":1,"entrant":{"id":24310999,"name":"DQ'd player"},
            "standing":{"stats":{"score":{"value":-1}}}}]}
      ]}}}}"#;

    #[test]
    fn reads_a_live_response_shape() {
        let b = parse(FIXTURE);
        assert_eq!(b.event_name, "Rivals of Aether II Singles");
        assert_eq!(b.tournament_name, "The Hangout #4");
        assert_eq!(b.sets.len(), 3);

        let gf = &b.sets[0];
        assert_eq!(gf.id, "106270113", "numeric ids stringify");
        assert!(!gf.preview);
        assert_eq!(gf.round, 6);
        assert_eq!(gf.station, Some(1));
        assert_eq!(gf.stream.as_deref(), Some("socalrivals"));
        assert_eq!(gf.bracket_type, "DOUBLE_ELIMINATION");
        assert_eq!(gf.phase_name, "Bracket");
        assert!(gf.is_complete());
        assert!(gf.is_ready());
        assert_eq!(gf.winner_slot(), Some(0));
        assert_eq!(gf.slots[0].score_text(), "3");
        assert_eq!(gf.slots[1].score_text(), "1");
    }

    #[test]
    fn keeps_unseeded_sets_instead_of_dropping_them() {
        // The whole point of this query over `available_sets`: a set whose
        // seats are still undetermined is a real node in the tree, not noise.
        let b = parse(FIXTURE);
        let lf = &b.sets[1];
        assert!(lf.preview, "placeholder id is flagged");
        assert!(!lf.is_ready(), "neither seat is filled yet");
        assert_eq!(lf.slots[0].name, None);
        assert_eq!(lf.slots[0].score_text(), "", "no score before a result");
        assert_eq!(lf.winner_slot(), None);
    }

    #[test]
    fn a_negative_score_is_a_dq_not_a_game_count() {
        let b = parse(FIXTURE);
        let lr1 = &b.sets[2];
        assert!(lr1.slots[1].is_dq());
        assert_eq!(lr1.slots[1].score_text(), "DQ");
        assert!(!lr1.slots[0].is_dq());
    }
}
