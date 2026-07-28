//! Unauthenticated start.gg website-API lookups (no token) — used by the
//! app's onboarding to echo back what an event URL points at before binding
//! to it. Kept separate from [`crate::startgg`], which is the operator's
//! token-authed client.

use serde_json::{json, Value};
use std::time::Duration;

const STARTGG_API: &str = "https://www.start.gg/api/-/gql";
// The website API 403s obvious library user agents; this mirrors the broker.
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Mobile Safari/537.36";

/// Resolve a pasted start.gg URL (or bare slug) to
/// `{ slug, name, tournament, entrants }` so a wrong paste is caught at
/// setup time instead of at the first set.
pub fn event_summary(url_or_slug: &str) -> Result<Value, String> {
    let slug = crate::forwarder::normalize_slug(url_or_slug);
    if !slug.starts_with("tournament/") {
        return Err("Expected a start.gg event URL like start.gg/tournament/…/event/…".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let body = json!({
        "query": "query($slug:String!){ event(slug:$slug){ name numEntrants tournament{ name } } }",
        "variables": { "slug": slug },
    });
    let res = client
        .post(STARTGG_API)
        .header("Content-Type", "application/json")
        .header("client-version", "20")
        .header("User-Agent", USER_AGENT)
        .json(&body)
        .send()
        .map_err(|e| format!("start.gg unreachable: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("start.gg HTTP {}", res.status()));
    }
    let out: Value = res
        .json()
        .map_err(|_| "start.gg returned invalid JSON".to_string())?;
    let ev = &out["data"]["event"];
    if ev.is_null() {
        return Err("No event at that URL — check the link.".into());
    }
    Ok(json!({
        "slug": slug,
        "name": ev["name"],
        "tournament": ev["tournament"]["name"],
        "entrants": ev["numEntrants"],
    }))
}
