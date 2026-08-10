//! Station-measured set times, read from the per-set journals the station
//! writes as each set finalizes: `<out dir>/sets/set_<id>.json`.
//!
//! These files are the DURABLE record: one file per set, written once, never
//! rewritten — they survive app restarts, re-ingests, and whatever happens to
//! the hub's mutable state. (`hub-state.json` records, by contrast, are
//! re-keyed and re-bound on every ingest, so a restart or a later session can
//! override them — the splitter must never depend on it.)
//!
//! A set file carries the station's own save-diff-measured `startEpoch` /
//! `endEpoch` — when games actually started and ended — plus each player's
//! character. What it does NOT carry is a start.gg set id (matching happens
//! on the hub), so joining to fetched start.gg sets works in two passes:
//!
//!   1. Exact, when available: if this machine's `hub-state.json` still holds
//!      a record for a set file (`set.setId` → `matchedStartggSetId`), that
//!      link is used verbatim. Purely opportunistic — a missing or stale hub
//!      state just means pass 2.
//!   2. Fuzzy, always sound: a set file matches a start.gg set when its
//!      measured window sits inside the start.gg click-window (Start Match is
//!      pressed before play starts, results are submitted after it ends —
//!      with tolerance for sloppy clicking), and the characters recorded on
//!      both sides don't contradict each other. Greedy best-overlap-first,
//!      one-to-one.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::sets::SetInfo;

/// How far a station-measured edge may fall outside the start.gg click-window
/// and still count as "inside" — a TO clicking Start Match a beat late is
/// normal; a set that started ten minutes before the click is a different set.
const EDGE_TOLERANCE_S: i64 = 300;

/// One finalized set as the station journaled it.
#[derive(Debug, Clone)]
pub struct LocalSet {
    /// The station's own set id (`set_<id>.json`'s `setId`).
    pub set_id: String,
    pub start_epoch: i64,
    pub end_epoch: i64,
    /// Characters seen in the set, for corroborating a time match.
    pub characters: Vec<String>,
}

/// Resolve where this app's own set journals live: the configured out dir
/// (or its default sibling of the config dir), plus `sets/`.
pub fn default_sets_dir(config_dir: &Path, cfg_out_dir: &str) -> PathBuf {
    let out = if cfg_out_dir.is_empty() {
        config_dir.join("matchlogger-out")
    } else {
        PathBuf::from(cfg_out_dir)
    };
    out.join("sets")
}

/// Accept either the out dir or its `sets/` child from a folder picker.
pub fn normalize_picked_dir(picked: PathBuf) -> PathBuf {
    if picked.file_name().map(|n| n == "sets").unwrap_or(false) {
        return picked;
    }
    let child = picked.join("sets");
    if child.is_dir() {
        child
    } else {
        picked
    }
}

/// Read every `set_*.json` in a sets folder. A missing folder is just "no
/// sets yet" (empty), not an error; individual unreadable files are skipped —
/// one corrupt journal must not hide the rest.
pub fn load_sets_dir(dir: &Path) -> Vec<LocalSet> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("set_") || !name.ends_with(".json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let (Some(start), Some(end)) = (
            v.get("startEpoch").and_then(Value::as_i64),
            v.get("endEpoch").and_then(Value::as_i64),
        ) else {
            continue;
        };
        if start <= 0 || end <= start {
            continue;
        }
        let set_id = match v.get("setId") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        let characters: Vec<String> = v
            .get("players")
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter()
                    .filter_map(|p| p.get("character").and_then(Value::as_str))
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        out.push(LocalSet {
            set_id,
            start_epoch: start,
            end_epoch: end,
            characters,
        });
    }
    out.sort_by_key(|s| s.start_epoch);
    out
}

/// Best-effort map of station set id → matched start.gg set id, read from a
/// `hub-state.json` if one exists. Anything missing, stale, or malformed
/// yields an empty map — the fuzzy pass covers those sets.
pub fn hub_links(hub_state_path: &Path) -> HashMap<String, String> {
    let mut links = HashMap::new();
    let Ok(text) = std::fs::read_to_string(hub_state_path) else {
        return links;
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return links;
    };
    let Some(buckets) = root.get("sets").and_then(Value::as_object) else {
        return links;
    };
    for bucket in buckets.values() {
        let Some(records) = bucket.as_object() else {
            continue;
        };
        for rec in records.values() {
            let sgg = match rec.get("matchedStartggSetId") {
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                _ => continue,
            };
            let set_id = match rec.pointer("/set/setId") {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => continue,
            };
            links.insert(set_id, sgg);
        }
    }
    links
}

/// A station window "fits" a start.gg click-window when both measured edges
/// sit inside it, give or take the tolerance. The start.gg window may be
/// arbitrarily longer (a set someone forgot to submit); the measured one
/// never legitimately extends far past either click.
fn fits(local: &LocalSet, sgg_start: i64, sgg_end: i64) -> bool {
    local.start_epoch >= sgg_start - EDGE_TOLERANCE_S
        && local.end_epoch <= sgg_end + EDGE_TOLERANCE_S
}

/// Character sets contradict when both sides recorded characters and none
/// coincide. Either side empty = no evidence either way.
fn characters_contradict(local: &LocalSet, set: &SetInfo) -> bool {
    if local.characters.is_empty() {
        return false;
    }
    let sgg: HashSet<&str> = set
        .players
        .iter()
        .filter_map(|p| p.character.as_deref())
        .filter(|c| !c.is_empty())
        .collect();
    if sgg.is_empty() {
        return false;
    }
    !local.characters.iter().any(|c| sgg.contains(c.as_str()))
}

/// Overlay station-measured times onto fetched sets. Exact hub links first,
/// then fuzzy window-fit matches (greedy, tightest fit first, one-to-one).
/// Returns how many sets got the upgrade.
///
/// Idempotent: re-running (the screen reloads its sources every open) leaves
/// times and the count stable — an already-overlaid set re-matches its own
/// journal (zero slack beats every other candidate), it doesn't block or
/// steal from others.
pub fn overlay_times(
    sets: &mut [SetInfo],
    local: &[LocalSet],
    links: &HashMap<String, String>,
) -> usize {
    let mut merged = 0;
    let mut local_used = vec![false; local.len()];
    let mut set_used = vec![false; sets.len()];

    // Pass 1: exact links from hub state, where they still exist.
    let by_sgg_id: HashMap<&str, usize> = local
        .iter()
        .enumerate()
        .filter_map(|(i, ls)| links.get(&ls.set_id).map(|sgg| (sgg.as_str(), i)))
        .collect();
    for (si, set) in sets.iter_mut().enumerate() {
        let Some(id) = set.id.as_deref() else {
            continue;
        };
        if let Some(&i) = by_sgg_id.get(id) {
            if !local_used[i] {
                set.started_at = local[i].start_epoch;
                set.completed_at = local[i].end_epoch;
                set.precise = true;
                local_used[i] = true;
                set_used[si] = true;
                merged += 1;
            }
        }
    }

    // Pass 2: fuzzy. Collect every plausible (set, local) pair, tightest
    // fit first — the pair whose windows agree best wins its members.
    let mut candidates: Vec<(usize, usize, i64)> = Vec::new();
    for (si, set) in sets.iter().enumerate() {
        if set_used[si] {
            continue;
        }
        for (li, ls) in local.iter().enumerate() {
            if local_used[li] {
                continue;
            }
            if !fits(ls, set.started_at, set.completed_at) {
                continue;
            }
            if characters_contradict(ls, set) {
                continue;
            }
            // Slack: how much wider the click-window is than the measured
            // one. 0 = the TO clicked exactly on time (or this set was
            // already overlaid from this very journal — re-matching it).
            let slack =
                (ls.start_epoch - set.started_at).abs() + (set.completed_at - ls.end_epoch).abs();
            candidates.push((si, li, slack));
        }
    }
    candidates.sort_by_key(|&(_, _, slack)| slack);
    for (si, li, _) in candidates {
        if set_used[si] || local_used[li] {
            continue;
        }
        sets[si].started_at = local[li].start_epoch;
        sets[si].completed_at = local[li].end_epoch;
        sets[si].precise = true;
        set_used[si] = true;
        local_used[li] = true;
        merged += 1;
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vodsplit::sets::Player;

    fn local(id: &str, start: i64, end: i64, chars: &[&str]) -> LocalSet {
        LocalSet {
            set_id: id.into(),
            start_epoch: start,
            end_epoch: end,
            characters: chars.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn sgg(id: &str, start: i64, end: i64, chars: &[&str]) -> SetInfo {
        SetInfo {
            id: Some(id.into()),
            precise: false,
            started_at: start,
            completed_at: end,
            station: Some(1),
            full_round_text: None,
            players: chars
                .iter()
                .enumerate()
                .map(|(i, c)| Player {
                    name: format!("p{i}"),
                    character: Some(c.to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn loads_set_journals_from_a_dir() {
        let dir = std::env::temp_dir().join(format!("rsr-setfiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("set_20260307_181530.json"),
            serde_json::json!({
                "setId": "20260307_181530", "complete": true,
                "startEpoch": 1000, "endEpoch": 1500,
                "players": [
                    {"slot": 0, "name": "A", "character": "Fleet", "wins": 3},
                    {"slot": 1, "name": "B", "character": "Kragg", "wins": 1},
                ],
            })
            .to_string(),
        )
        .unwrap();
        // Interrupted journal: real times, still usable.
        std::fs::write(
            dir.join("set_20260307_190001_interrupted.json"),
            serde_json::json!({
                "setId": "20260307_190001", "complete": false,
                "startEpoch": 2000, "endEpoch": 2400, "players": [],
            })
            .to_string(),
        )
        .unwrap();
        // Noise that must be skipped: wrong prefix, corrupt, no end time.
        std::fs::write(dir.join("current.json"), "{}").unwrap();
        std::fs::write(dir.join("set_bad.json"), "not json").unwrap();
        std::fs::write(
            dir.join("set_x.json"),
            r#"{"setId":"x","startEpoch":5,"endEpoch":null}"#,
        )
        .unwrap();

        let got = load_sets_dir(&dir);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].set_id, "20260307_181530");
        assert_eq!(got[0].characters, vec!["Fleet", "Kragg"]);
        assert_eq!(
            got[1].set_id, "20260307_190001",
            "interrupted journal still loads"
        );

        assert!(
            load_sets_dir(&dir.join("nope")).is_empty(),
            "missing dir is just empty"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exact_hub_link_beats_fuzzy() {
        let mut sets = vec![sgg("111", 900, 2000, &[])];
        let local_sets = vec![local("a", 1000, 1500, &[])];
        let links = HashMap::from([("a".to_string(), "111".to_string())]);
        assert_eq!(overlay_times(&mut sets, &local_sets, &links), 1);
        assert!(sets[0].precise);
        assert_eq!((sets[0].started_at, sets[0].completed_at), (1000, 1500));
    }

    #[test]
    fn fuzzy_matches_a_contained_window() {
        // TO clicked Start at 900, submitted at 2000; games ran 1000-1500.
        let mut sets = vec![sgg("111", 900, 2000, &["Fleet", "Kragg"])];
        let local_sets = vec![local("a", 1000, 1500, &["Fleet", "Kragg"])];
        assert_eq!(overlay_times(&mut sets, &local_sets, &HashMap::new()), 1);
        assert_eq!((sets[0].started_at, sets[0].completed_at), (1000, 1500));
    }

    #[test]
    fn fuzzy_rejects_disjoint_windows_and_contradicting_characters() {
        let mut sets = vec![
            sgg("out-of-window", 5000, 6000, &[]),
            sgg("wrong-chars", 900, 2000, &["Zetterburn", "Orcane"]),
        ];
        let local_sets = vec![local("a", 1000, 1500, &["Fleet", "Kragg"])];
        assert_eq!(overlay_times(&mut sets, &local_sets, &HashMap::new()), 0);
    }

    #[test]
    fn fuzzy_is_one_to_one_and_prefers_the_tightest_fit() {
        // Two start.gg sets whose click-windows both contain the measured
        // window; the tighter one (real counterpart) wins, the never-closed
        // one stays imprecise instead of stealing it.
        let mut sets = vec![
            sgg("sloppy", 900, 5000, &[]), // forgot to submit
            sgg("tight", 950, 1600, &[]),
        ];
        let local_sets = vec![local("a", 1000, 1500, &[])];
        assert_eq!(overlay_times(&mut sets, &local_sets, &HashMap::new()), 1);
        assert!(!sets[0].precise);
        assert!(sets[1].precise);
    }

    #[test]
    fn overlay_is_idempotent_across_reloads() {
        let mut sets = vec![sgg("111", 900, 2000, &[]), sgg("222", 2100, 3000, &[])];
        let local_sets = vec![local("a", 1000, 1500, &[])];
        assert_eq!(overlay_times(&mut sets, &local_sets, &HashMap::new()), 1);
        // Screen reopened: sources reload, overlay runs again on the
        // already-overlaid sets. Same count, same times, no stealing.
        assert_eq!(overlay_times(&mut sets, &local_sets, &HashMap::new()), 1);
        assert_eq!((sets[0].started_at, sets[0].completed_at), (1000, 1500));
        assert!(!sets[1].precise);
    }

    #[test]
    fn sequential_sets_land_one_to_one() {
        // A normal evening: back-to-back sets on one station, click-windows
        // slightly padded around each measured window.
        let mut sets = vec![
            sgg("r1", 950, 1650, &[]),
            sgg("r2", 1700, 2500, &[]),
            sgg("r3", 2600, 3300, &[]),
        ];
        let local_sets = vec![
            local("a", 1000, 1600, &[]),
            local("b", 1800, 2400, &[]),
            local("c", 2700, 3200, &[]),
        ];
        assert_eq!(overlay_times(&mut sets, &local_sets, &HashMap::new()), 3);
        assert_eq!((sets[0].started_at, sets[0].completed_at), (1000, 1600));
        assert_eq!((sets[1].started_at, sets[1].completed_at), (1800, 2400));
        assert_eq!((sets[2].started_at, sets[2].completed_at), (2700, 3200));
    }

    #[test]
    fn picked_dir_normalizes_to_the_sets_child() {
        let dir = std::env::temp_dir().join(format!("rsr-setdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sets")).unwrap();
        assert_eq!(normalize_picked_dir(dir.clone()), dir.join("sets"));
        assert_eq!(normalize_picked_dir(dir.join("sets")), dir.join("sets"));
        let bare = std::env::temp_dir().join(format!("rsr-bare-{}", std::process::id()));
        assert_eq!(normalize_picked_dir(bare.clone()), bare);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
