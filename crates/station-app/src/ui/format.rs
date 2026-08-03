//! Pure formatting helpers — the Rust counterpart of operatorFormat.ts.

use serde_json::Value;

/// "12m" / "1h 03m" style elapsed label against a wall-clock now.
pub fn elapsed_since(started_s: i64, now_s: i64) -> String {
    let d = (now_s - started_s).max(0);
    let m = d / 60;
    if m < 1 {
        "now".into()
    } else if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h {:02}m", m / 60, m % 60)
    }
}

/// Local wall-clock HH:MM for a Unix epoch.
pub fn clock(epoch: i64) -> String {
    let dt = std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch.max(0) as u64);
    let local: chrono::DateTime<chrono::Local> = dt.into();
    local.format("%H:%M").to_string()
}

/// "first to N" from start.gg's totalGames (best-of-N).
pub fn best_of(total_games: Option<i64>) -> Option<String> {
    match total_games {
        Some(n) if n > 0 => Some(format!("first to {}", (n + 1) / 2)),
        _ => None,
    }
}

/// The "X vs Y" line for a hub record (players under record.set.players).
pub fn hub_players_label(rec: &Value) -> String {
    let players = rec
        .pointer("/set/players")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if players.is_empty() {
        // No games yet -> no station-side tags; the bracket entrants are the
        // only names there are. The raw set id is a last resort, never a
        // first impression.
        let entrants: Vec<String> = rec
            .get("entrants")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect();
        if !entrants.is_empty() {
            return entrants.join(" vs ");
        }
        return crate::model::id_str(rec.get("id").unwrap_or(&Value::Null));
    }
    players
        .iter()
        .map(|p| p.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string())
        .collect::<Vec<_>>()
        .join(" vs ")
}

/// The "3–2" score line for a hub record.
pub fn hub_score(rec: &Value) -> String {
    let players = rec
        .pointer("/set/players")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    players
        .iter()
        .map(|p| p.get("wins").and_then(|v| v.as_i64()).unwrap_or(0).to_string())
        .collect::<Vec<_>>()
        .join("–")
}

/// Public tag database tooltip — same wording as the web app's sggTitle.
pub fn sgg_title(tag: &str, sgg: &str) -> String {
    format!(
        "Public tag database: \"{tag}\" is registered to start.gg @{sgg}. \
         Not a bracket match, just what was submitted."
    )
}
