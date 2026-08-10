//! start.gg GraphQL client — the UNAUTHENTICATED website API, no personal
//! token needed (the same endpoint TournamentStreamHelper uses). Set times,
//! stations, and character selections are all public bracket data; requiring
//! every TO to mint an API key for that was pure friction.
//!
//! (The old token-authed endpoint this replaced was also plain wrong —
//! api.start.gg/graphql is a 404; the real one is api.start.gg/gql/alpha —
//! so token-auth here never worked anyway.)

use serde::Deserialize;

const ENDPOINT: &str = "https://www.start.gg/api/-/gql";
// The website API 403s obvious library user agents; same string as
// station-core's `startgg_web` (the onboarding lookup).
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Mobile Safari/537.36";
const PER_PAGE: u32 = 50;

/// start.gg caps how much you can pull per minute; a big bracket is a lot of
/// pages, so give each request room rather than failing the whole fetch.
const TIMEOUT_SECS: u64 = 30;

const QUERY: &str = r#"
query EventSets($slug: String!, $page: Int!, $perPage: Int!) {
  event(slug: $slug) {
    id
    name
    tournament { name }
    sets(page: $page, perPage: $perPage, sortType: RECENT) {
      pageInfo { totalPages }
      nodes {
        id
        startedAt
        completedAt
        fullRoundText
        station { number }
        games {
          selections {
            entrant { name }
            character { name }
          }
        }
      }
    }
  }
}
"#;

#[derive(Debug, Clone)]
pub struct Player {
    pub name: String,
    pub character: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SetInfo {
    /// start.gg's set id (stringified), the join key for reporter data.
    pub id: Option<String>,
    pub started_at: i64,
    pub completed_at: i64,
    pub station: Option<i64>,
    pub full_round_text: Option<String>,
    pub players: Vec<Player>,
    /// True once the times were overlaid from a reporter hub-state file
    /// (station-measured), rather than start.gg's click timestamps.
    pub precise: bool,
}

#[derive(Debug, Clone)]
pub struct EventSets {
    pub event_name: String,
    pub tournament_name: String,
    pub sets: Vec<SetInfo>,
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
    #[serde(rename = "startedAt")]
    started_at: Option<i64>,
    #[serde(rename = "completedAt")]
    completed_at: Option<i64>,
    #[serde(rename = "fullRoundText")]
    full_round_text: Option<String>,
    station: Option<RawStation>,
    games: Option<Vec<RawGame>>,
}

#[derive(Deserialize)]
struct RawStation {
    number: Option<i64>,
}

#[derive(Deserialize)]
struct RawGame {
    selections: Option<Vec<RawSelection>>,
}

#[derive(Deserialize)]
struct RawSelection {
    entrant: Option<RawEntrant>,
    character: Option<RawCharacter>,
}

#[derive(Deserialize)]
struct RawEntrant {
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawCharacter {
    name: Option<String>,
}

/// Pull the `tournament/x/event/y` part out of a full URL, or accept a bare
/// slug that's already in that form.
pub fn parse_slug(input: &str) -> Option<String> {
    let s = input.trim().trim_matches('/');
    let start = s.find("tournament/")?;
    let rest = &s[start..];
    // Cut anything after the event segment (query strings, /brackets/..., etc.)
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 4 || parts[2] != "event" {
        return None;
    }
    let event = parts[3].split(['?', '#']).next()?;
    if parts[1].is_empty() || event.is_empty() {
        return None;
    }
    Some(format!("tournament/{}/event/{}", parts[1], event))
}

/// Each set's players, taken from the first game that actually recorded
/// selections. Characters are only present for games that report them, so this
/// keeps the entrant name either way and de-dupes repeats across selections.
fn players_of(set: &RawSet) -> Vec<Player> {
    let games = match &set.games {
        Some(g) => g,
        None => return Vec::new(),
    };
    let mut out: Vec<Player> = Vec::new();
    for game in games {
        let Some(selections) = &game.selections else {
            continue;
        };
        for sel in selections {
            let Some(entrant) = &sel.entrant else {
                continue;
            };
            let Some(name) = entrant.name.clone() else {
                continue;
            };
            let character = sel.character.as_ref().and_then(|c| c.name.clone());
            match out.iter_mut().find(|p| p.name == name) {
                // Seen this entrant already — only worth revisiting to fill in a
                // character we didn't have yet.
                Some(existing) => {
                    if existing.character.is_none() {
                        existing.character = character;
                    }
                }
                None => out.push(Player { name, character }),
            }
        }
        // Two players is a normal set; stop once we have both with characters.
        if out.len() >= 2 && out.iter().all(|p| p.character.is_some()) {
            break;
        }
    }
    out
}

/// Fetch every completed set in an event, following pagination.
pub async fn fetch_sets(slug: String) -> Result<EventSets, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let mut all: Vec<SetInfo> = Vec::new();
    let mut event_name = String::new();
    let mut tournament_name = String::new();
    let mut page = 1u32;
    let mut total_pages = 1u32;

    while page <= total_pages {
        let body = serde_json::json!({
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

        let event = parsed
            .data
            .and_then(|d| d.event)
            .ok_or_else(|| "No event at that slug — check the URL.".to_string())?;

        if event_name.is_empty() {
            event_name = event.name.clone().unwrap_or_default();
            tournament_name = event
                .tournament
                .as_ref()
                .and_then(|t| t.name.clone())
                .unwrap_or_default();
        }

        let sets = event.sets.ok_or_else(|| "Event has no sets.".to_string())?;
        if page == 1 {
            total_pages = sets
                .page_info
                .as_ref()
                .and_then(|p| p.total_pages)
                .unwrap_or(1)
                .max(1);
        }

        for node in sets.nodes.unwrap_or_default() {
            // Only sets with both timestamps can be turned into a clip.
            let (Some(started), Some(completed)) = (node.started_at, node.completed_at) else {
                continue;
            };
            let players = players_of(&node);
            // ids arrive as JSON numbers; stringified so the reporter join
            // compares like for like.
            let id = node.id.as_ref().map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            all.push(SetInfo {
                id,
                started_at: started,
                completed_at: completed,
                station: node.station.as_ref().and_then(|s| s.number),
                full_round_text: node.full_round_text.clone(),
                players,
                precise: false,
            });
        }
        page += 1;
    }

    all.sort_by_key(|s| s.started_at);
    Ok(EventSets {
        event_name,
        tournament_name,
        sets: all,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slugs() {
        let want = Some("tournament/the-hangout-1/event/rivals-2".to_string());
        assert_eq!(
            parse_slug("https://www.start.gg/tournament/the-hangout-1/event/rivals-2"),
            want
        );
        // trailing path (a bracket link) and query strings are ignored
        assert_eq!(
            parse_slug("https://start.gg/tournament/the-hangout-1/event/rivals-2/brackets/1/2"),
            want
        );
        assert_eq!(
            parse_slug("tournament/the-hangout-1/event/rivals-2?foo=1"),
            want
        );
        assert_eq!(parse_slug("not a slug"), None);
        assert_eq!(parse_slug("tournament/only-a-tournament"), None);
    }
}
