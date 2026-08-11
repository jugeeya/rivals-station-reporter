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
/// and still count as "inside". Two real effects set the size: a TO clicking
/// Start Match late, and — the dominant one — a journal's `endEpoch` being
/// the FINALIZE time, which lags the last game by up to the station's idle
/// window (420s by default) when no next set starts sooner. At The Hangout
/// 4.1 every station-1 journal overshot start.gg's `completedAt` by ~400s;
/// a 300s tolerance would have missed nearly the whole event.
const EDGE_TOLERANCE_S: i64 = 600;

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

/// Character evidence agrees when the smaller of the two character sets is
/// contained in the larger. Mere overlap is not enough: at The Hangout 4.1 a
/// Kragg/Wrastor journal time-fit a Fleet/Kragg set on another station more
/// tightly than its own set — one shared character must not carry a match.
/// The containment direction tolerates one-sided gaps (start.gg often has
/// selections for only some games; a journal can list one character for a
/// set whose start.gg record has two). Either side empty = no evidence.
fn characters_agree(local: &LocalSet, set: &SetInfo) -> bool {
    let ours: HashSet<&str> = local.characters.iter().map(String::as_str).collect();
    let sgg: HashSet<&str> = set
        .players
        .iter()
        .filter_map(|p| p.character.as_deref())
        .filter(|c| !c.is_empty())
        .collect();
    if ours.is_empty() || sgg.is_empty() {
        return true;
    }
    let (small, big) = if ours.len() <= sgg.len() {
        (&ours, &sgg)
    } else {
        (&sgg, &ours)
    };
    small.iter().all(|c| big.contains(c))
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
            if !characters_agree(ls, set) {
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

    /// Case study: station 1's whole night at The Hangout 4.1
    /// (tournament/the-hangout-4-1/event/rivals-of-aether-ii-singles,
    /// 2026-08-08) — all 38 of the event's start.gg sets and all 20 journals
    /// the station wrote, real timestamps and real character data. Two things
    /// this proved the hard way:
    ///
    /// * Every journal's `endEpoch` overshot start.gg's `completedAt` by
    ///   ~400s (the idle-finalize window) — the original 300s tolerance
    ///   would have missed nearly every set of the event.
    /// * Journal 021037 time-fits a station-4 set MORE TIGHTLY than its own
    ///   set; only the character contradiction (Fleet/Kragg vs
    ///   Etalus/Zetterburn) keeps the match honest.
    #[test]
    fn hangout41_station1_matches_every_bracket_set_and_no_friendlies() {
        // (start.gg set id, station, startedAt, completedAt, characters)
        #[rustfmt::skip]
        let event: &[(&str, i64, i64, i64, &[&str])] = &[
            ("106270083", 2, 1786231708, 1786232941, &["Olympia"][..]),
            ("106270087", 3, 1786231819, 1786232850, &["Absa", "Fleet"][..]),
            ("106270091", 4, 1786231824, 1786233118, &["Clairen", "Zetterburn"][..]),
            ("106270103", 1, 1786232049, 1786233499, &["Fleet", "Wrastor"][..]),
            ("106270095", 3, 1786232950, 1786234027, &["Gouie", "Kragg"][..]),
            ("106270098", 2, 1786233033, 1786233910, &["La Reina", "Olympia"][..]),
            ("106270099", 4, 1786233136, 1786233924, &["Forsburn", "Loxodont"][..]),
            ("106270105", 1, 1786233524, 1786234282, &["Etalus", "Ranno"][..]),
            ("106270157", 4, 1786233943, 1786235373, &["Fleet"][..]),
            ("106270102", 2, 1786233987, 1786234778, &["Kragg", "Zetterburn"][..]),
            ("106270101", 3, 1786234183, 1786235051, &["Fleet", "Olympia"][..]),
            ("106270106", 1, 1786234359, 1786235625, &["La Reina", "Loxodont"][..]),
            ("106270155", 2, 1786234820, 1786236064, &["Olympia", "Ranno"][..]),
            ("106270100", 3, 1786235086, 1786235850, &["Absa", "Zetterburn"][..]),
            ("106270108", 1, 1786235656, 1786236503, &["Kragg", "Wrastor"][..]),
            ("106270104", 3, 1786235861, 1786236870, &["Fleet", "Kragg"][..]),
            ("106270161", 2, 1786236110, 1786236908, &["Forsburn", "Gouie", "Ranno"][..]),
            ("106270164", 4, 1786236141, 1786236980, &["Fleet", "Zetterburn"][..]),
            ("106270107", 1, 1786236888, 1786237819, &["Fleet", "Zetterburn"][..]),
            ("106270159", 2, 1786236971, 1786237757, &["Clairen", "Olympia"][..]),
            ("106270166", 3, 1786237015, 1786238653, &["Forsburn", "Olympia"][..]),
            ("106270163", 4, 1786237228, 1786239086, &["Kragg", "Olympia"][..]),
            ("106270165", 2, 1786237788, 1786238416, &["Absa", "La Reina", "Olympia"][..]),
            ("106270109", 1, 1786237833, 1786239024, &["Etalus", "Fleet"][..]),
            ("106270168", 2, 1786238474, 1786239667, &["Fleet", "Loxodont"][..]),
            ("106270169", 1, 1786239146, 1786239939, &["Absa", "Etalus"][..]),
            ("106270167", 2, 1786239189, 1786240273, &["Fleet", "Kragg"][..]),
            ("106270170", 3, 1786239193, 1786240122, &["Fleet", "Kragg"][..]),
            ("106270110", 1, 1786239960, 1786241172, &["La Reina", "Loxodont", "Zetterburn"][..]),
            ("106270172", 4, 1786240144, 1786240915, &["Etalus", "Wrastor"][..]),
            ("106270171", 3, 1786240298, 1786241517, &["Fleet", "Loxodont"][..]),
            ("106270111", 1, 1786241180, 1786242211, &["Fleet", "Kragg"][..]),
            ("106270174", 4, 1786241290, 1786242136, &["Etalus", "Zetterburn"][..]),
            ("106270173", 1, 1786242264, 1786243235, &["Fleet", "Kragg"][..]),
            ("106270112", 1, 1786243230, 1786244339, &["Fleet", "Loxodont"][..]),
            ("106270175", 1, 1786244337, 1786245465, &["Kragg", "Zetterburn"][..]),
            ("106270176", 1, 1786245480, 1786246253, &["Fleet", "Kragg"][..]),
            ("106270113", 1, 1786246264, 1786247392, &["Kragg", "Loxodont"][..]),
        ];
        // (journal set id, startEpoch, endEpoch, characters, expected match).
        // The last seven are post-bracket friendlies and side sets — real
        // games with real times that must match NOTHING.
        type Journal = (
            &'static str,
            i64,
            i64,
            &'static [&'static str],
            Option<&'static str>,
        );
        #[rustfmt::skip]
        let journals: &[Journal] = &[
            ("20260809_000628", 1786233988, 1786234690, &["Etalus", "Ranno"][..], Some("106270105")),
            ("20260809_002111", 1786234871, 1786236035, &["Loxodont"][..], Some("106270106")),
            ("20260809_004034", 1786236034, 1786236911, &["Kragg", "Wrastor"][..], Some("106270108")),
            ("20260809_010250", 1786237370, 1786238148, &["Fleet", "Zetterburn"][..], Some("106270107")),
            ("20260809_011547", 1786238147, 1786239391, &["Etalus", "Fleet"][..], Some("106270109")),
            ("20260809_013844", 1786239524, 1786240359, &["Absa", "Etalus"][..], Some("106270169")),
            ("20260809_015414", 1786240454, 1786241438, &["Loxodont", "Zetterburn"][..], Some("106270110")),
            ("20260809_021037", 1786241437, 1786242569, &["Fleet", "Kragg"][..], Some("106270111")),
            ("20260809_022928", 1786242568, 1786243587, &["Fleet", "Kragg"][..], Some("106270173")),
            ("20260809_024725", 1786243645, 1786244720, &["Fleet", "Loxodont"][..], Some("106270112")),
            ("20260809_030537", 1786244737, 1786245821, &["Kragg", "Zetterburn"][..], Some("106270175")),
            ("20260809_032339", 1786245819, 1786246645, &["Fleet", "Kragg"][..], Some("106270176")),
            ("20260809_033905", 1786246745, 1786247778, &["Kragg", "Loxodont"][..], Some("106270113")),
            ("20260809_040556", 1786248356, 1786249747, &["Fleet", "Ranno"][..], None),
            ("20260809_042924", 1786249764, 1786251224, &["Fleet", "Olympia"][..], None),
            ("20260809_045643", 1786251403, 1786252605, &["Gouie"][..], None),
            ("20260809_051643", 1786252603, 1786253848, &["Ranno", "Zetterburn"][..], None),
            ("20260809_054350", 1786254230, 1786254865, &["Wrastor", "Zetterburn"][..], None),
            ("20260809_055358", 1786254838, 1786255486, &["Forsburn", "Slade"][..], None),
            ("20260809_060517", 1786255517, 1786260627, &["Fleet", "Zetterburn"][..], None),
        ];

        let mut sets: Vec<SetInfo> = event
            .iter()
            .map(|&(id, station, start, end, chars)| SetInfo {
                station: Some(station),
                ..sgg(id, start, end, chars)
            })
            .collect();
        let local_sets: Vec<LocalSet> = journals
            .iter()
            .map(|&(id, start, end, chars, _)| local(id, start, end, chars))
            .collect();

        let matched = overlay_times(&mut sets, &local_sets, &HashMap::new());
        assert_eq!(matched, 13, "every bracket journal lands, no friendly does");

        for &(jid, start, end, _, expected) in journals {
            match expected {
                Some(sgg_id) => {
                    let s = sets
                        .iter()
                        .find(|s| s.id.as_deref() == Some(sgg_id))
                        .unwrap();
                    assert!(
                        s.precise && s.started_at == start && s.completed_at == end,
                        "journal {jid} should own set {sgg_id} \
                         (got precise={} {}..{})",
                        s.precise,
                        s.started_at,
                        s.completed_at,
                    );
                }
                None => {
                    // A friendly's measured window must not appear on any set.
                    assert!(
                        !sets
                            .iter()
                            .any(|s| s.started_at == start && s.completed_at == end),
                        "friendly journal {jid} was matched to a bracket set"
                    );
                }
            }
        }
    }
}
