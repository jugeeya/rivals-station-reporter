//! Stream overlay output: `overlay.html` + `overlay-data.js` in the output
//! dir, for an OBS Browser source (local file). The page is static; only the
//! data file is rewritten, and only when its content changes, so the page can
//! poll it without flicker. A `<script src>` load (rather than fetch) is used
//! because it works from a `file://` page.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};
use station_core::set_machine::Tally;

const OVERLAY_HTML: &str = include_str!("../../assets/overlay.html");

/// The data file body: `window.RSR_OVERLAY = {...};`. `updatedAt` is left out
/// of `body` so the caller can compare content without it changing every tick.
///
/// `record` is the session's win/loss line for the streamer, taken to be the
/// tag with the most games (they're in every one; opponents rotate).
pub fn data_body(
    session_games: u64,
    tallies: &HashMap<String, Tally>,
    snapshot: &Value,
    station: i64,
) -> Value {
    let record = tallies
        .iter()
        .filter(|(_, t)| t.games() > 0)
        .max_by(|(an, a), (bn, b)| a.games().cmp(&b.games()).then_with(|| bn.cmp(an)))
        .map(|(tag, t)| {
            json!({
                "tag": tag,
                "gameWins": t.game_wins, "gameLosses": t.game_losses,
                "setWins": t.set_wins, "setLosses": t.set_losses,
            })
        });
    let live = snapshot.get("live").filter(|l| !l.is_null()).map(|l| {
        let players: Vec<Value> = l["players"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|p| json!({ "tag": p["tag"], "wins": p["wins"].as_i64().unwrap_or(0) }))
                    .collect()
            })
            .unwrap_or_default();
        json!({ "players": players, "games": l["games"] })
    });
    json!({
        "sessionGames": session_games,
        "station": station,
        "record": record,
        "live": live,
    })
}

/// Make sure `overlay.html` is current and write the data file if `body`
/// differs from `last_written` (updated on a write). Also rewrites at least
/// every 10s so the page's staleness check can tell the app is still running.
pub fn write(
    out_dir: &Path,
    body: &Value,
    last_written: &mut Option<(String, i64)>,
) -> std::io::Result<()> {
    let now = station_core::now_sec();
    let body_str = body.to_string();
    if let Some((prev, at)) = last_written {
        if *prev == body_str && now - *at < 10 {
            return Ok(());
        }
    }
    std::fs::create_dir_all(out_dir)?;
    let html_path = out_dir.join("overlay.html");
    if std::fs::read_to_string(&html_path).ok().as_deref() != Some(OVERLAY_HTML) {
        std::fs::write(&html_path, OVERLAY_HTML)?;
    }
    let mut full = body.clone();
    full["updatedAt"] = json!(now);
    let js = format!("window.RSR_OVERLAY = {};\n", full);
    let data_path = out_dir.join("overlay-data.js");
    let tmp = out_dir.join("overlay-data.js.tmp");
    std::fs::write(&tmp, js)?;
    std::fs::rename(&tmp, &data_path)?;
    *last_written = Some((body_str, now));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_carries_count_and_live_score() {
        let snap = json!({
            "history": [],
            "live": {"games": 2, "players": [
                {"tag": "A", "char": "Fleet", "wins": 1, "slot": 0},
                {"tag": "B", "char": "Kragg", "wins": 1, "slot": 1},
            ]},
        });
        let b = data_body(7, &HashMap::new(), &snap, 3);
        assert_eq!(b["sessionGames"], 7);
        assert_eq!(b["live"]["players"][1]["tag"], "B");
        assert_eq!(b["live"]["players"][0]["wins"], 1);
    }

    #[test]
    fn record_is_the_tag_with_most_games() {
        let mut t = HashMap::new();
        t.insert("ME".to_string(), Tally { game_wins: 3, game_losses: 1, set_wins: 1, set_losses: 0 });
        t.insert("A".to_string(), Tally { game_wins: 1, game_losses: 2, set_wins: 0, set_losses: 1 });
        t.insert("B".to_string(), Tally { game_wins: 0, game_losses: 1, set_wins: 0, set_losses: 0 });
        let b = data_body(4, &t, &json!({"live": null}), 1);
        assert_eq!(b["record"]["tag"], "ME");
        assert_eq!(b["record"]["gameWins"], 3);
        assert_eq!(b["record"]["setWins"], 1);
        assert!(data_body(0, &HashMap::new(), &json!({}), 1)["record"].is_null());
    }

    #[test]
    fn no_live_set_is_null() {
        let b = data_body(0, &HashMap::new(), &json!({"history": [], "live": null}), 1);
        assert!(b["live"].is_null());
    }

    #[test]
    fn writes_html_and_data_once_until_content_changes() {
        let dir = std::env::temp_dir().join(format!("overlaytest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut last = None;
        let b = data_body(1, &HashMap::new(), &json!({"live": null}), 1);
        write(&dir, &b, &mut last).unwrap();
        assert!(dir.join("overlay.html").is_file());
        let js = std::fs::read_to_string(dir.join("overlay-data.js")).unwrap();
        assert!(js.starts_with("window.RSR_OVERLAY = {"));
        assert!(js.contains("\"sessionGames\":1"));
        let b2 = data_body(2, &HashMap::new(), &json!({"live": null}), 1);
        write(&dir, &b2, &mut last).unwrap();
        let js2 = std::fs::read_to_string(dir.join("overlay-data.js")).unwrap();
        assert!(js2.contains("\"sessionGames\":2"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
