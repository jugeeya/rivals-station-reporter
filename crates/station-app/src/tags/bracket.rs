//! Bracket entrant lookup: resolve a start.gg event URL to its entrants and
//! their linked user slugs, for matching against the published tag manifest.
//! Same unauthenticated website endpoint as the VOD splitter's set fetcher.

use serde_json::json;

const STARTGG_API: &str = "https://www.start.gg/api/-/gql";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Mobile Safari/537.36";

#[derive(Debug, Clone)]
pub struct Entrant {
    /// The entrant name as it appears in the bracket (may carry a team prefix).
    pub entrant: String,
    pub gamer_tag: String,
    /// Linked start.gg user slug (`user/6192f6f1`) — the manifest join key.
    pub slug: String,
}

#[derive(Debug, Clone)]
pub struct EventEntrants {
    pub event: String,
    pub entrants: Vec<Entrant>,
}

fn str_at<'a>(v: &'a serde_json::Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(|x| x.as_str()).unwrap_or("")
}

async fn gql(query: &str, variables: serde_json::Value) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let res = client
        .post(STARTGG_API)
        .header("Content-Type", "application/json")
        .header("client-version", "20")
        .header("User-Agent", USER_AGENT)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("start.gg returned {}", res.status()));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let has_errors = body
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if has_errors {
        return Err("start.gg GraphQL error".into());
    }
    Ok(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
}

/// Fetch every entrant with a linked start.gg account. Entrants without a
/// linked user can't be matched to published tags and are omitted (they'll
/// show up as misses by name only if someone published under their slug —
/// i.e. never — so the caller counts them out of the match entirely).
pub async fn event_entrants(url: String) -> Result<EventEntrants, String> {
    let slug = crate::vodsplit::sets::parse_slug(&url)
        .ok_or_else(|| "Expected a start.gg event URL (…/tournament/<t>/event/<e>).".to_string())?;

    let mut entrants = Vec::new();
    let mut event_name = String::new();
    let mut page = 1i64;
    let mut total_pages = 1i64;

    while page <= total_pages && page <= 30 {
        let data = gql(
            "query($slug:String!,$page:Int!){ event(slug:$slug){ name \
               entrants(query:{ page:$page, perPage:64 }){ \
                 pageInfo{ totalPages } \
                 nodes{ name participants{ gamerTag user{ slug } } } } } }",
            json!({ "slug": slug, "page": page }),
        )
        .await?;

        let ev = data
            .get("event")
            .filter(|v| !v.is_null())
            .ok_or_else(|| "No event at that URL — check the link.".to_string())?;

        if page == 1 {
            event_name = str_at(ev, "/name").to_string();
        }
        total_pages = ev
            .pointer("/entrants/pageInfo/totalPages")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);

        if let Some(nodes) = ev.pointer("/entrants/nodes").and_then(|v| v.as_array()) {
            for n in nodes {
                let entrant = str_at(n, "/name").to_string();
                if let Some(parts) = n.pointer("/participants").and_then(|v| v.as_array()) {
                    for p in parts {
                        let slug = str_at(p, "/user/slug");
                        if !slug.is_empty() {
                            entrants.push(Entrant {
                                entrant: entrant.clone(),
                                gamer_tag: str_at(p, "/gamerTag").to_string(),
                                slug: slug.to_string(),
                            });
                        }
                    }
                }
            }
        }
        page += 1;
    }

    Ok(EventEntrants {
        event: event_name,
        entrants,
    })
}
