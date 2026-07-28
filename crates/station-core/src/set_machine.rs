//! Set state machine + stats producer — port of the bottom half of
//! `rivals_stats.py` (`_SetMachine`, `StatsProducer`).
//!
//! Groups per-game results into sets and writes the same JSON files the old
//! UE4SS mod wrote (`current.json`, `live.json`, `sets/*.json`) into an output
//! dir the app owns. A GameN reset (a fresh Game1), an idle timeout, or
//! shutdown bounds a set. The forwarder then ships those files — and the
//! snapshot feeds the UI directly.
//!
//! `StatsProducer` treats a game's replay as required, not optional: the save
//! flushes a game's stats before its replay file is visible on disk (see the
//! comments on `StatsProducer::poll`), so a freshly-detected game is held as
//! `pending` until its replay shows up (or a timeout forces it through
//! without one), rather than being recorded on the spot with an incomplete
//! picture.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::stats::{char_full, iso_of, stamp_of, GameResult, Replay};
use crate::{now_sec, stats};

pub type LogFn = Box<dyn Fn(&str) + Send + Sync>;
pub type OnChangeFn = Box<dyn Fn(&Value) + Send + Sync>;

/// The open set being accumulated (Python's `self.set` dict).
struct OpenSet {
    id: String,
    start_epoch: i64,
    start_iso: String,
    first_match_start_iso: Option<String>,
    matches: Vec<Value>,
    wins_by_name: HashMap<String, i64>,
    /// LOCAL / ONLINE / RANKED, straight from the save. Only LOCAL is a
    /// tournament game; the rest are shown but never reported.
    mode: Option<String>,
}

pub struct SetMachine {
    out: PathBuf,
    sets_dir: PathBuf,
    log: LogFn,
    on_change: Option<OnChangeFn>,
    set: Option<OpenSet>,
    history: Vec<Value>,
    last_game_at: i64,
}

impl SetMachine {
    pub fn new(out_dir: &Path, log: LogFn, on_change: Option<OnChangeFn>) -> std::io::Result<Self> {
        let sets_dir = out_dir.join("sets");
        std::fs::create_dir_all(&sets_dir)?;
        Ok(Self {
            out: out_dir.to_path_buf(),
            sets_dir,
            log,
            on_change,
            set: None,
            history: Vec::new(),
            last_game_at: 0,
        })
    }

    fn summ(s: &OpenSet, complete: bool) -> Value {
        let last = s.matches.last().expect("summ requires matches");
        let mut players: Vec<Value> = last["players"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|p| {
                json!({
                    "tag": p["name"], "char": p["character"],
                    "wins": p["wins"], "slot": p["slot"],
                })
            })
            .collect();
        players.sort_by_key(|p| p["slot"].as_i64().unwrap_or(i64::MAX));
        let top = players
            .iter()
            .filter_map(|p| p["wins"].as_i64())
            .max()
            .unwrap_or(0);
        let unique = players
            .iter()
            .filter(|p| p["wins"].as_i64() == Some(top))
            .count()
            == 1;
        for p in &mut players {
            let won = p["wins"].as_i64() == Some(top) && top > 0 && unique;
            p["won"] = json!(won);
        }
        json!({
            "startEpoch": s.start_epoch, "complete": complete,
            "mode": s.mode, "games": s.matches.len(), "players": players,
        })
    }

    /// `{history: [...], live: summ|null}` — what the UI renders.
    pub fn snapshot(&self) -> Value {
        let live = match &self.set {
            Some(s) if !s.matches.is_empty() => Self::summ(s, false),
            _ => Value::Null,
        };
        json!({ "history": self.history, "live": live })
    }

    fn emit(&self) {
        if let Some(cb) = &self.on_change {
            cb(&self.snapshot());
        }
    }

    /// Atomic JSON write (tmp + rename), pretty-printed like the Python's
    /// `indent=2` — external tooling may be tailing these files.
    fn write(&self, name: &Path, obj: &Value) {
        let path = if name.is_absolute() {
            name.to_path_buf()
        } else {
            self.out.join(name)
        };
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(obj).unwrap_or_else(|_| "{}".into());
        if std::fs::write(&tmp, body)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .is_err()
        {
            (self.log)(&format!("could not write {}", path.display()));
        }
    }

    pub fn write_current(&self, obj: &Value) {
        self.write(Path::new("current.json"), obj);
    }

    pub fn write_live(&self, obj: &Value) {
        self.write(Path::new("live.json"), obj);
    }

    fn start(&mut self, at: i64) {
        self.set = Some(OpenSet {
            id: stamp_of(at),
            start_epoch: at,
            start_iso: iso_of(at),
            first_match_start_iso: None,
            matches: Vec::new(),
            wins_by_name: HashMap::new(),
            mode: None,
        });
    }

    /// Characters leave here as full names ("Clairen"), never save codes
    /// ("Cla") — the hub's start.gg character lookup, the console, and Discord
    /// all work in full names. `char_full()` is idempotent on names it doesn't
    /// know, so codes and full names compare safely below.
    fn players(result: &GameResult, replay: Option<&Replay>) -> Vec<Value> {
        let mut players: Vec<Value> = result
            .winners
            .iter()
            .map(|k| (k, true))
            .chain(result.losers.iter().map(|k| (k, false)))
            .map(|(k, won)| {
                json!({
                    "name": k.tag, "character": char_full(&k.char_code),
                    "won": won, "stats": k.stats,
                })
            })
            .collect();

        // ONLINE: only the local tag came from the save -> opponent from replay.
        if players.len() == 1 {
            if let Some(rep) = replay {
                if rep.players.len() == 2 {
                    let mine = players[0].clone();
                    let opp = rep
                        .players
                        .iter()
                        .find(|p| char_full(&p.char_code) != mine["character"])
                        .or_else(|| rep.players.iter().find(|p| p.name.clone() != mine["name"]));
                    if let Some(opp) = opp {
                        let won = !mine["won"].as_bool().unwrap_or(false);
                        players.push(json!({
                            "name": opp.name, "character": char_full(&opp.char_code),
                            "won": won, "stats": {},
                        }));
                    }
                }
            }
        }

        // Slots from the replay, by character; leftovers by position.
        for (i, pl) in players.iter_mut().enumerate() {
            let mut slot: Option<usize> = None;
            if let Some(rep) = replay {
                for (idx, rp) in rep.players.iter().enumerate() {
                    if char_full(&rp.char_code) == pl["character"] {
                        slot = Some(idx);
                        break;
                    }
                }
            }
            pl["slot"] = json!(slot.unwrap_or(i));
        }
        players
    }

    pub fn record_game(&mut self, result: &GameResult, replay: Option<&Replay>, at: i64) {
        let game_num = replay.map(|r| r.game);
        let end_epoch = replay.map(|r| r.epoch).unwrap_or(at);
        if game_num == Some(1) && self.set.as_ref().is_some_and(|s| !s.matches.is_empty()) {
            self.finalize(true, Some(at));
        }
        if self.set.is_none() {
            self.start(end_epoch);
        }
        let set = self.set.as_mut().expect("just started");
        if !result.mode.is_empty() {
            set.mode = Some(result.mode.clone());
        }

        let players = Self::players(result, replay);
        let winner_name = players
            .iter()
            .find(|p| p["won"].as_bool() == Some(true))
            .and_then(|p| p["name"].as_str())
            .map(|s| s.to_string());
        if let Some(w) = &winner_name {
            *set.wins_by_name.entry(w.clone()).or_insert(0) += 1;
        }

        let match_players: Vec<Value> = players
            .iter()
            .map(|p| {
                let name = p["name"].as_str().unwrap_or_default();
                let mut row = json!({
                    "slot": p["slot"], "name": p["name"], "character": p["character"],
                    "wins": set.wins_by_name.get(name).copied().unwrap_or(0),
                });
                // row.update(p['stats']) — per-match stats merge in flat.
                if let (Some(obj), Some(st)) = (row.as_object_mut(), p["stats"].as_object()) {
                    for (k, v) in st {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                row
            })
            .collect();

        let record = json!({
            "index": set.matches.len() + 1, "startTime": null, "startEpoch": null,
            "endTime": iso_of(end_epoch), "endEpoch": end_epoch, "durationSeconds": null,
            "playerCount": match_players.len(), "gameNumber": game_num,
            "mode": result.mode, "players": match_players,
        });
        if set.first_match_start_iso.is_none() {
            set.first_match_start_iso = Some(iso_of(end_epoch));
        }
        set.matches.push(record);
        self.last_game_at = now_sec();

        let set = self.set.as_ref().unwrap();
        let standings: Vec<Value> = set.matches.last().unwrap()["players"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| json!({"slot": p["slot"], "name": p["name"], "character": p["character"], "wins": p["wins"]}))
            .collect();
        self.write_live(&json!({
            "setId": set.id, "complete": false, "winsRequired": null,
            "mode": set.mode, "matchCount": set.matches.len(),
            "players": standings, "matches": set.matches,
        }));
        self.write_current(&json!({
            "state": "set_open", "epoch": end_epoch, "setId": set.id,
            "matchCount": set.matches.len(),
        }));
        let vs = standings
            .iter()
            .map(|p| {
                format!(
                    "{}({})",
                    p["name"].as_str().unwrap_or("?"),
                    p["character"].as_str().unwrap_or("?")
                )
            })
            .collect::<Vec<_>>()
            .join(" vs ");
        let scores = standings
            .iter()
            .map(|p| format!("{} {}", p["name"].as_str().unwrap_or("?"), p["wins"]))
            .collect::<Vec<_>>()
            .join(" ");
        (self.log)(&format!(
            "game {} | {} -> {} wins [{}]",
            game_num
                .map(|g| g.to_string())
                .unwrap_or_else(|| "?".into()),
            vs,
            winner_name.as_deref().unwrap_or("?"),
            scores
        ));
        self.emit();
    }

    pub fn finalize(&mut self, complete: bool, at: Option<i64>) {
        let Some(set) = self.set.take() else { return };
        if set.matches.is_empty() {
            return;
        }
        let at = at.unwrap_or_else(now_sec);
        let standings: Vec<Value> = set.matches.last().unwrap()["players"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| json!({"slot": p["slot"], "name": p["name"], "character": p["character"], "wins": p["wins"]}))
            .collect();
        let winner = standings
            .iter()
            .max_by_key(|p| p["wins"].as_i64().unwrap_or(i64::MIN))
            .cloned();
        let report = json!({
            "setId": set.id, "complete": complete,
            "startTime": set.start_iso, "startEpoch": set.start_epoch,
            "firstMatchStartTime": set.first_match_start_iso,
            "endTime": iso_of(at), "endEpoch": at,
            "durationSeconds": if set.start_epoch != 0 { json!(at - set.start_epoch) } else { Value::Null },
            "winsRequired": null, "mode": set.mode,
            "matchCount": set.matches.len(),
            "winnerSlot": winner.as_ref().map(|w| w["slot"].clone()).unwrap_or(Value::Null),
            "winnerName": winner.as_ref().map(|w| w["name"].clone()).unwrap_or(Value::Null),
            "winnerCharacter": winner.as_ref().map(|w| w["character"].clone()).unwrap_or(Value::Null),
            "players": standings, "matches": set.matches, "source": "stats-diff",
        });
        let fname = format!(
            "set_{}{}.json",
            set.id,
            if complete { "" } else { "_interrupted" }
        );
        self.write(&self.sets_dir.join(&fname), &report);
        (self.log)(&format!(
            "finalized {}: {} wins {} ({} games, complete={})",
            fname,
            winner
                .as_ref()
                .and_then(|w| w["name"].as_str())
                .unwrap_or("?"),
            standings
                .iter()
                .map(|s| s["wins"].to_string())
                .collect::<Vec<_>>()
                .join("-"),
            set.matches.len(),
            complete
        ));
        self.history.push(Self::summ(&set, complete));
        self.write_current(&json!({"state": "idle", "epoch": at}));
        self.write_live(&json!({"complete": true}));
        self.emit();
    }

    pub fn idle_check(&mut self, idle_s: f64) {
        if self.set.is_some()
            && self.last_game_at > 0
            && (now_sec() - self.last_game_at) as f64 > idle_s
        {
            (self.log)("idle timeout - finalizing open set");
            self.finalize(true, None);
        }
    }

    /// True while a set is open with at least one game in it.
    pub fn has_open_set(&self) -> bool {
        self.set.as_ref().is_some_and(|s| !s.matches.is_empty())
    }
}

/// A game whose save-diff has already been detected but which cannot be
/// recorded yet because its replay isn't on disk (see the race documented on
/// `poll()`). Held until the replay shows up or `PENDING_TIMEOUT_S` elapses.
struct PendingGame {
    result: GameResult,
    /// The epoch the game was detected at — used both as the replay-lookup
    /// anchor and, if nothing else, the timestamp it's eventually recorded
    /// under.
    at: i64,
}

/// Wires the save/replays to the SetMachine. Call `poll()` each tick.
pub struct StatsProducer {
    save: PathBuf,
    replays: PathBuf,
    idle_s: f64,
    log_armed: bool,
    pub machine: SetMachine,
    mtime: Option<std::time::SystemTime>,
    baseline: Option<HashMap<String, f64>>,
    /// At most one game at a time waiting on its replay to appear on disk.
    pending: Option<PendingGame>,
}

impl StatsProducer {
    /// The save writes a game's stats, then its replay file becomes visible
    /// on disk anywhere from a few seconds to ~15s later (measured: every
    /// .rpl's mtime lands 9-16s after the timestamp encoded in its own
    /// filename). This window has to comfortably cover that lag on both
    /// sides, since detection time and replay time can each drift a little.
    pub const REPLAY_WINDOW_S: i64 = 45;

    /// Safety valve for `pending`: if a replay still hasn't shown up after
    /// this long, stop waiting and record the game without one (today's old
    /// behaviour) rather than holding it — and the data it protects —
    /// forever.
    pub const PENDING_TIMEOUT_S: i64 = 90;

    pub fn new(
        save_path: &Path,
        replays_dir: &Path,
        out_dir: &Path,
        idle_s: f64,
        log: LogFn,
        on_change: Option<OnChangeFn>,
    ) -> std::io::Result<Self> {
        let machine = SetMachine::new(out_dir, log, on_change)?;
        let mut p = Self {
            save: save_path.to_path_buf(),
            replays: replays_dir.to_path_buf(),
            idle_s,
            log_armed: false,
            machine,
            mtime: None,
            baseline: None,
            pending: None,
        };
        p.baseline = p.read();
        if let Some(base) = &p.baseline {
            p.machine
                .write_current(&json!({"state": "idle", "epoch": now_sec()}));
            (p.machine.log)(&format!(
                "stats source armed | tags: {}",
                crate::save::tag_names(base).join(", ")
            ));
            p.log_armed = true;
        } else {
            (p.machine.log)(&format!(
                "WARNING: could not read save at {}",
                p.save.display()
            ));
        }
        Ok(p)
    }

    /// True once the save has been read successfully at least once.
    pub fn armed(&self) -> bool {
        self.baseline.is_some()
    }

    fn read(&self) -> Option<HashMap<String, f64>> {
        let bytes = std::fs::read(&self.save).ok()?;
        crate::save::parse_stats(&bytes).ok()
    }

    fn newest_replay_near(&self, epoch: i64) -> Option<Replay> {
        let mut best: Option<Replay> = None;
        let entries = std::fs::read_dir(&self.replays).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.ends_with(".rpl") {
                continue;
            }
            let Some(parsed) = stats::parse_replay_name(name) else {
                continue;
            };
            if (parsed.epoch - epoch).abs() > Self::REPLAY_WINDOW_S {
                continue;
            }
            if best.as_ref().is_none_or(|b| parsed.epoch > b.epoch) {
                best = Some(parsed);
            }
        }
        best
    }

    /// Try to record `pending` right now: with its replay if one has shown up
    /// on disk, or — only when `force` — without one. Returns whether it was
    /// recorded (i.e. whether the caller can stop holding it).
    fn try_record_pending(&mut self, pending: &PendingGame, force: bool) -> bool {
        if let Some(replay) = self.newest_replay_near(pending.at) {
            self.machine
                .record_game(&pending.result, Some(&replay), pending.at);
            return true;
        }
        if force {
            (self.machine.log)(&format!(
                "no replay matched game at {} within {}s — recording without opponent/GameN (unmatched)",
                pending.at,
                Self::PENDING_TIMEOUT_S
            ));
            self.machine.record_game(&pending.result, None, pending.at);
            return true;
        }
        false
    }

    /// Resolve the pending game if possible: record it once its replay is on
    /// disk, or force it through unmatched once `PENDING_TIMEOUT_S` has
    /// elapsed (the safety valve — a game is never silently lost). No-op if
    /// nothing is pending or it's still within the wait window.
    fn resolve_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let timed_out = now_sec() - pending.at > Self::PENDING_TIMEOUT_S;
        if !self.try_record_pending(&pending, timed_out) {
            self.pending = Some(pending); // still waiting
        }
    }

    pub fn poll(&mut self) {
        // Resolve a pending game on EVERY poll, including ticks where the
        // save didn't change — the replay we're waiting on can land at any
        // point relative to our poll cadence, not just the tick a new game
        // was detected on.
        self.resolve_pending();

        // Never idle-finalize while a game is pending: the open set is
        // missing its most recent game until the pending one lands, so
        // finalizing now would close the set moments early and split what
        // should be one set into two.
        if self.pending.is_none() {
            self.machine.idle_check(self.idle_s);
        }

        let Ok(meta) = std::fs::metadata(&self.save) else {
            return;
        };
        let Ok(mtime) = meta.modified() else { return };
        if self.mtime == Some(mtime) {
            return;
        }
        self.mtime = Some(mtime);
        let Some(nxt) = self.read() else { return }; // mid-write; retry next poll
        let Some(baseline) = self.baseline.take() else {
            self.baseline = Some(nxt);
            return;
        };
        let result = stats::to_game_result(stats::diff(&baseline, &nxt));
        // The baseline must advance here regardless of what happens to
        // `result` below — the diff is only computable against consecutive
        // save reads, so this can't be redone once we've moved past it.
        self.baseline = Some(nxt);
        let Some(result) = result else { return }; // non-match write
        let at = now_sec();

        // THE RACE (measured from production data, not theorised): the save
        // flushes a game's stats to disk before that game's replay file
        // becomes visible — a game detected at 15:39:08 had its replay named
        // ...15-39-07... (a second earlier, well inside the window) yet the
        // lookup found nothing, because every .rpl's mtime lands 9-16s after
        // the timestamp encoded in its own filename. Recording immediately
        // with "whatever replay we can find right now" therefore regularly
        // finds nothing — and for an ONLINE/RANKED game (whose opponent is
        // ONLY recoverable from the replay) or set grouping (whose GameN
        // ONLY comes from the replay), that permanently loses the opponent +
        // character, or splits one real set into several one-game "sets".
        // So: never record straight off a diff. Hold it as pending and only
        // call record_game once a replay is actually found, or the
        // PENDING_TIMEOUT_S safety valve fires.
        if let Some(prev) = self.pending.take() {
            // Only one game is pending in practice, but if a second save
            // change lands before the first resolves, flush the older one
            // first (forced, replay or not) so game order is preserved and
            // neither game is silently dropped.
            self.try_record_pending(&prev, true);
        }
        self.pending = Some(PendingGame { result, at });
        // Try to resolve the fresh pending game immediately in case its
        // replay is already sitting on disk (e.g. after a slow poll tick).
        self.resolve_pending();
    }

    pub fn shutdown(&mut self) {
        self.machine.finalize(false, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{diff, parse_replay_name, to_game_result};
    use std::collections::HashMap as Map;

    type StatRow<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, f64)]);

    fn snap(rows: &[StatRow]) -> Map<String, f64> {
        let cat: Map<&str, &str> = [
            ("matches", "MatchesByCharacter"),
            ("wins", "WinsByCharacter"),
            ("losses", "LossesByCharacter"),
            ("kos", "KOsByCharacter"),
            ("deaths", "DeathsByCharacter"),
            ("damageDealt", "DamageDealtByCharacter"),
            ("damageTaken", "DamageTakenByCharacter"),
            ("grabSuccesses", "GrabSuccessesByCharacter"),
        ]
        .into_iter()
        .collect();
        let mut flat = Map::new();
        for (tag, ch, mode, stats) in rows {
            for (k, v) in *stats {
                flat.insert(format!("{tag}|{ch}|{mode}|{}", cat[k]), *v);
            }
        }
        flat
    }

    /// The end-to-end shape test from test_rivals_stats.py: two real games ->
    /// one open set -> finalize writes the mod-contract set file.
    #[test]
    fn set_machine_end_to_end() {
        let g1 = snap(&[
            ("KIM", "Zet", "LOCAL", &[("matches", 1.0), ("wins", 1.0)]),
            ("KIM", "Random", "LOCAL", &[("matches", 1.0), ("wins", 1.0)]),
            (
                "JUGZ!",
                "Fle",
                "LOCAL",
                &[("matches", 1.0), ("losses", 1.0)],
            ),
            (
                "JUGZ!",
                "Random",
                "LOCAL",
                &[("matches", 1.0), ("losses", 1.0)],
            ),
        ]);
        let g2 = snap(&[
            (
                "JUGZ!",
                "Gal",
                "LOCAL",
                &[
                    ("matches", 1.0),
                    ("wins", 1.0),
                    ("kos", 3.0),
                    ("damageDealt", 83.0),
                    ("grabSuccesses", 3.0),
                ],
            ),
            (
                "JUGZ!",
                "Random",
                "LOCAL",
                &[("matches", 1.0), ("wins", 1.0)],
            ),
            (
                "KIM",
                "Zet",
                "LOCAL",
                &[
                    ("matches", 1.0),
                    ("losses", 1.0),
                    ("deaths", 3.0),
                    ("damageTaken", 83.0),
                ],
            ),
            (
                "KIM",
                "Random",
                "LOCAL",
                &[("matches", 1.0), ("losses", 1.0)],
            ),
        ]);
        let r1 = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let r2 = to_game_result(diff(&Map::new(), &g2)).unwrap();
        let rep1 = parse_replay_name("2026-07-23_19-52-44-606-Player1(Fle)-Player2(Zet)-Game1.rpl")
            .unwrap();
        let rep2 = parse_replay_name("2026-07-23_19-56-58-454-Player1(Gal)-Player2(Zet)-Game2.rpl")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        m.record_game(&r1, Some(&rep1), rep1.epoch);
        m.record_game(&r2, Some(&rep2), rep2.epoch);

        // both games in ONE open set
        assert!(m.has_open_set());
        let matches = &m.set.as_ref().unwrap().matches;
        assert_eq!(matches.len(), 2);

        // slots from replay: JUGZ!(Gal)->0, KIM(Zet)->1; cumulative wins 1-1;
        // per-match stats merged into the record
        let g2rec = &matches[1];
        let players = g2rec["players"].as_array().unwrap();
        let jz = players.iter().find(|p| p["name"] == "JUGZ!").unwrap();
        let km = players.iter().find(|p| p["name"] == "KIM").unwrap();
        assert_eq!(
            (jz["slot"].as_i64(), km["slot"].as_i64()),
            (Some(0), Some(1))
        );
        assert_eq!(
            (jz["wins"].as_i64(), km["wins"].as_i64()),
            (Some(1), Some(1))
        );
        assert_eq!(jz["kos"].as_i64(), Some(3));
        assert_eq!(jz["damageDealt"].as_i64(), Some(83));

        // full names, not save codes
        assert_eq!(jz["character"], "Galvan");
        assert_eq!(km["character"], "Zetterburn");

        m.finalize(true, Some(rep2.epoch + 5));
        let files: Vec<_> = std::fs::read_dir(dir.join("sets"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 1);
        assert!(files[0].starts_with("set_") && files[0].ends_with(".json"));

        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("sets").join(&files[0])).unwrap(),
        )
        .unwrap();
        for k in [
            "setId",
            "complete",
            "startEpoch",
            "endEpoch",
            "winsRequired",
            "matchCount",
            "winnerSlot",
            "winnerName",
            "winnerCharacter",
            "players",
            "matches",
        ] {
            assert!(saved.get(k).is_some(), "set file missing {k}");
        }
        assert_eq!(saved["matchCount"].as_i64(), Some(2));
        assert_eq!(saved["mode"], "LOCAL");

        let live: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("live.json")).unwrap()).unwrap();
        assert_eq!(live["complete"], true);
        let current: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("current.json")).unwrap())
                .unwrap();
        assert_eq!(current["state"], "idle");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh Game1 while a set is open finalizes the previous set first.
    #[test]
    fn game1_reset_bounds_the_set() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let rep_a = parse_replay_name("2026-07-23_19-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();
        let rep_b = parse_replay_name("2026-07-23_20-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();

        let dir = std::env::temp_dir().join(format!("statstest2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        m.record_game(&r, Some(&rep_a), rep_a.epoch);
        m.record_game(&r, Some(&rep_b), rep_b.epoch); // fresh Game1
        assert!(m.has_open_set());
        assert_eq!(m.set.as_ref().unwrap().matches.len(), 1); // new set has just the new game
        let files = std::fs::read_dir(dir.join("sets")).unwrap().count();
        assert_eq!(files, 1); // previous set finalized to disk
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The snapshot marks winners only when the top score is unique.
    #[test]
    fn snapshot_won_flags() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let rep = parse_replay_name("2026-07-23_19-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();
        let dir = std::env::temp_dir().join(format!("statstest3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        m.record_game(&r, Some(&rep), rep.epoch);
        let snap = m.snapshot();
        let live = &snap["live"];
        assert_eq!(live["complete"], false);
        assert_eq!(live["mode"], "LOCAL");
        let players = live["players"].as_array().unwrap();
        let a = players.iter().find(|p| p["tag"] == "A").unwrap();
        let b = players.iter().find(|p| p["tag"] == "B").unwrap();
        assert_eq!(a["won"], true);
        assert_eq!(b["won"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `StatsProducer` rooted at a fresh temp dir, pointed at a save path
    /// that doesn't exist. That's fine for the pending-replay tests below:
    /// they drive `pending` and `poll()` directly rather than through a real
    /// save-file diff, so `poll()` only ever gets as far as resolving/timing
    /// out the pending game before `std::fs::metadata` on the missing save
    /// bails it out early.
    fn producer(dir: &Path, replays: &Path, idle_s: f64) -> StatsProducer {
        StatsProducer::new(
            &dir.join("missing.sav"),
            replays,
            &dir.join("out"),
            idle_s,
            Box::new(|_| {}),
            None,
        )
        .unwrap()
    }

    /// A replay filename timestamped at (approximately) "now", in the local
    /// wall-clock format the game itself writes. `parse_replay_name` reads
    /// filenames as local time, so round-tripping through the machine's own
    /// local clock lands back within a second of `now_sec()` — close enough
    /// for these tests, which only need "recent enough to not be timed out
    /// and close enough to match the REPLAY_WINDOW_S window".
    fn now_replay_name(a: &str, ca: &str, b: &str, cb: &str, game: i64) -> String {
        let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-000");
        format!("{ts}-{a}({ca})-{b}({cb})-Game{game}.rpl")
    }

    /// No replay on disk yet: the pending game must not be recorded, and
    /// must still be waiting afterwards.
    #[test]
    fn pending_without_replay_records_nothing_yet() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend1_{}", std::process::id()));
        let replays = dir.join("replays"); // left empty — replay hasn't landed yet
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 180.0);
        // `at` is "just now" — well inside PENDING_TIMEOUT_S, so this must
        // stay pending rather than being forced through unmatched.
        p.pending = Some(PendingGame {
            result: r,
            at: now_sec(),
        });

        p.poll();

        assert!(
            !p.machine.has_open_set(),
            "no replay yet -> nothing should be recorded"
        );
        assert!(p.pending.is_some(), "the game must still be held pending");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Once a matching replay appears on disk, the next poll records the
    /// game — recovering the opponent (ONLINE games only move the local
    /// tag's stats) and the GameN from the replay filename.
    #[test]
    fn pending_resolves_once_replay_appears_with_opponent_and_gamenum() {
        // Simulates an ONLINE game: only the local tag's stats moved in the
        // save, so the opponent is only knowable from the replay.
        let g_online = snap(&[("ME", "Fle", "ONLINE", &[("wins", 1.0)])]);
        let r = to_game_result(diff(&Map::new(), &g_online)).unwrap();
        let fname = now_replay_name("ME", "Fle", "OPP", "Kra", 1);
        let rep = parse_replay_name(&fname).unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend2_{}", std::process::id()));
        let replays = dir.join("replays");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 180.0);
        p.pending = Some(PendingGame {
            result: r,
            at: rep.epoch,
        });

        p.poll(); // replay still missing -> stays pending
        assert!(!p.machine.has_open_set());

        // The replay lands on disk (mirrors the measured 9-16s lag).
        std::fs::write(replays.join(fname), []).unwrap();

        p.poll(); // now it can be resolved
        assert!(p.pending.is_none());
        assert!(p.machine.has_open_set());

        let matches = &p.machine.set.as_ref().unwrap().matches;
        assert_eq!(matches.len(), 1);
        let players = matches[0]["players"].as_array().unwrap();
        assert_eq!(players.len(), 2, "opponent recovered from the replay");
        let opp = players.iter().find(|pl| pl["name"] == "OPP").unwrap();
        assert_eq!(opp["character"], "Kragg");
        assert_eq!(matches[0]["gameNumber"].as_i64(), Some(1));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The safety valve: a pending game older than PENDING_TIMEOUT_S is
    /// eventually recorded without a replay rather than lost forever.
    #[test]
    fn pending_past_timeout_is_recorded_without_replay() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend3_{}", std::process::id()));
        let replays = dir.join("replays"); // left empty — no replay will ever show up
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 180.0);
        let at = now_sec() - StatsProducer::PENDING_TIMEOUT_S - 1;
        p.pending = Some(PendingGame { result: r, at });

        p.poll(); // no replay, but past the timeout -> forced through

        assert!(p.pending.is_none());
        assert!(p.machine.has_open_set());
        let matches = &p.machine.set.as_ref().unwrap().matches;
        assert_eq!(matches[0]["gameNumber"], Value::Null); // no replay -> no GameN

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// idle_check must not finalize the open set while a game is pending —
    /// otherwise the set could close moments before the pending game lands
    /// and would get split in two.
    #[test]
    fn idle_check_is_suppressed_while_a_game_is_pending() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let rep = parse_replay_name("2026-07-23_19-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend4_{}", std::process::id()));
        let replays = dir.join("replays");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        // idle_s tiny enough that it would fire immediately if not guarded.
        let mut p = producer(&dir, &replays, 0.001);
        p.machine.record_game(&r, Some(&rep), rep.epoch);
        p.machine.last_game_at = now_sec() - 10; // well past idle_s

        // A second game's diff just landed but its replay hasn't yet.
        let r2 = to_game_result(diff(&Map::new(), &g1)).unwrap();
        p.pending = Some(PendingGame {
            result: r2,
            at: now_sec(),
        });

        p.poll(); // idle_s has clearly elapsed, but a game is pending

        assert!(
            p.machine.has_open_set(),
            "set must stay open while a game is pending"
        );
        let sets_written = std::fs::read_dir(dir.join("out").join("sets"))
            .unwrap()
            .count();
        assert_eq!(sets_written, 0, "idle_check must not have fired");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// If a second save change arrives while one game is still pending, the
    /// older one is flushed (forced, replay or not) before the new one is
    /// stored — order is preserved and neither game is dropped.
    #[test]
    fn second_pending_game_flushes_the_first_in_order() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r1 = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let r2 = to_game_result(diff(&Map::new(), &g1)).unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend5_{}", std::process::id()));
        let replays = dir.join("replays"); // no replays land for either game
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 180.0);
        p.pending = Some(PendingGame {
            result: r1,
            at: now_sec() - 5,
        });
        // Directly exercises poll()'s "flush the old pending game first"
        // branch by taking the existing pending game and forcing it through
        // exactly as poll() would when a fresh result arrives.
        let prev = p.pending.take().unwrap();
        p.try_record_pending(&prev, true);
        p.pending = Some(PendingGame {
            result: r2,
            at: now_sec(),
        });

        assert!(p.machine.has_open_set());
        let matches = &p.machine.set.as_ref().unwrap().matches;
        assert_eq!(
            matches.len(),
            1,
            "the first pending game was flushed, unmatched"
        );
        assert!(
            p.pending.is_some(),
            "the second game is now the one pending"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
