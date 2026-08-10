//! Station-measured set times, read from a hub-state.json.
//!
//! This app's hub stores every set it ingested with the STATION'S OWN
//! start/end times — derived from save-file diffing, so they mark when games
//! actually started and ended, not when a TO remembered to click Start Match /
//! Submit on start.gg. Each record also carries `matchedStartggSetId`, the
//! exact start.gg set it was bound to, which makes joining the two sources
//! exact rather than fuzzy: fetch the bracket from start.gg as usual, then
//! overlay the hub's times wherever a set id matches.
//!
//! On the operator PC the file is simply this app's own
//! `<config dir>/hub-state.json`; a picked file covers splitting on some
//! other machine (copy the file over — it's small).

use std::collections::HashMap;
use std::path::Path;

use super::sets::SetInfo;

#[derive(Debug, Clone)]
pub struct ReporterSet {
    /// start.gg's set id, stringified (they're numbers on the wire).
    pub startgg_set_id: String,
    pub start_epoch: i64,
    pub end_epoch: i64,
}

/// Parse a reporter `hub-state.json`. Shape (see rivals-station-reporter's
/// hub.rs): `{ "sets": { "<event slug>": { "<station:setId>": record } } }`,
/// where each record has `matchedStartggSetId` and a `set` summary with
/// `startEpoch`/`endEpoch`. Records that never matched a bracket set, or
/// whose set never got both timestamps, can't improve anything and are
/// skipped.
pub fn load_hub_state(path: &Path) -> Result<Vec<ReporterSet>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let root: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| "That file isn't a reporter hub-state.json.".to_string())?;
    let buckets = root
        .get("sets")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "That file isn't a reporter hub-state.json.".to_string())?;

    let mut out = Vec::new();
    for bucket in buckets.values() {
        let Some(records) = bucket.as_object() else {
            continue;
        };
        for rec in records.values() {
            let id = match rec.get("matchedStartggSetId") {
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(serde_json::Value::String(s)) if !s.is_empty() => s.clone(),
                _ => continue,
            };
            let start = rec.pointer("/set/startEpoch").and_then(|v| v.as_i64());
            let end = rec.pointer("/set/endEpoch").and_then(|v| v.as_i64());
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };
            if start <= 0 || end <= start {
                continue;
            }
            out.push(ReporterSet {
                startgg_set_id: id,
                start_epoch: start,
                end_epoch: end,
            });
        }
    }
    Ok(out)
}

/// Overlay reporter times onto fetched sets, joined by start.gg set id.
/// Returns how many sets got the upgrade. Untouched sets keep start.gg's
/// click-times and stay `precise: false`.
pub fn merge_times(sets: &mut [SetInfo], reporter: &[ReporterSet]) -> usize {
    let by_id: HashMap<&str, &ReporterSet> = reporter
        .iter()
        .map(|r| (r.startgg_set_id.as_str(), r))
        .collect();
    let mut merged = 0;
    for set in sets.iter_mut() {
        let Some(id) = set.id.as_deref() else {
            continue;
        };
        if let Some(r) = by_id.get(id) {
            set.started_at = r.start_epoch;
            set.completed_at = r.end_epoch;
            set.precise = true;
            merged += 1;
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vodsplit::sets::SetInfo;

    fn hub_state_fixture() -> String {
        serde_json::json!({
            "version": 3,
            "stations": {},
            "sets": {
                "tournament/x/event/y": {
                    "1:20260101_010101": {
                        "matchedStartggSetId": 111,
                        "set": { "startEpoch": 1000, "endEpoch": 1600 },
                    },
                    // never matched a bracket set -> useless for the join
                    "2:20260101_020202": {
                        "matchedStartggSetId": null,
                        "set": { "startEpoch": 2000, "endEpoch": 2600 },
                    },
                    // matched but the set never closed out -> no end time
                    "3:20260101_030303": {
                        "matchedStartggSetId": 333,
                        "set": { "startEpoch": 3000, "endEpoch": null },
                    },
                }
            }
        })
        .to_string()
    }

    fn write_fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("vodsplit-reporter-tests");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(format!("hub-state-{}.json", std::process::id()));
        std::fs::write(&p, hub_state_fixture()).unwrap();
        p
    }

    fn set(id: Option<&str>, started: i64, completed: i64) -> SetInfo {
        SetInfo {
            id: id.map(str::to_string),
            started_at: started,
            completed_at: completed,
            station: Some(1),
            full_round_text: None,
            players: Vec::new(),
            precise: false,
        }
    }

    #[test]
    fn loads_only_usable_records() {
        let p = write_fixture();
        let got = load_hub_state(&p).unwrap();
        assert_eq!(got.len(), 1, "unmatched and endless records are skipped");
        assert_eq!(got[0].startgg_set_id, "111");
        assert_eq!((got[0].start_epoch, got[0].end_epoch), (1000, 1600));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn merge_overlays_matched_sets_only() {
        let reporter = vec![ReporterSet {
            startgg_set_id: "111".into(),
            start_epoch: 1000,
            end_epoch: 1600,
        }];
        let mut sets = vec![
            set(Some("111"), 900, 2000), // start.gg's sloppy click times
            set(Some("222"), 5000, 5600),
            set(None, 7000, 7600),
        ];
        assert_eq!(merge_times(&mut sets, &reporter), 1);
        assert_eq!((sets[0].started_at, sets[0].completed_at), (1000, 1600));
        assert!(sets[0].precise);
        assert_eq!((sets[1].started_at, sets[1].completed_at), (5000, 5600));
        assert!(!sets[1].precise);
        assert!(!sets[2].precise);
    }

    #[test]
    fn rejects_non_hub_state_files() {
        let dir = std::env::temp_dir().join("vodsplit-reporter-tests");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(format!("not-hub-{}.json", std::process::id()));
        std::fs::write(&p, "{\"hello\": 1}").unwrap();
        assert!(load_hub_state(&p).is_err());
        let _ = std::fs::remove_file(p);
    }
}
