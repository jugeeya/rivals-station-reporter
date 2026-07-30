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
//!
//! The open set and any pending game live only in memory until the set
//! finalizes - a crash or restart before then would otherwise silently lose
//! every game played so far (the stats-diff that produced them can't be
//! replayed). Both are journaled to `open-set.json` in the output dir
//! whenever either changes, and reloaded on startup; see the "Journal"
//! section near the bottom of this file for the on-disk shape and the
//! staleness rule that decides whether a found journal is resumed or
//! finalized as a completed set.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::stats::{char_full, iso_of, stamp_of, GameResult, Replay, SidePlayer};
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
    /// When the last game was recorded, for `idle_check`'s elapsed-time math
    /// only - a `std::time::Instant`, not a wall-clock epoch, so an NTP step
    /// or a manual clock change mid-set can't corrupt the idle timer (a
    /// forward jump would otherwise falsely finalize an open set early; a
    /// backward jump would make the delta negative and idle would never
    /// fire at all). Nothing here is ever written to an output file, so
    /// there's no wall-clock meaning to preserve.
    last_game_at: Option<std::time::Instant>,
    /// Wall-clock companion of `last_game_at`, set at the same moment (see
    /// `record_game`). Unlike `last_game_at` this IS meaningful across a
    /// restart, and exists solely so the journal has something to reconstitute
    /// `last_game_at` from - an `Instant` from a previous process is garbage,
    /// but "this many wall-clock seconds have passed" survives the restart
    /// intact and can rebuild an equivalent `Instant` on the other side.
    last_game_at_wall: Option<i64>,
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
            last_game_at: None,
            last_game_at_wall: None,
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
        self.last_game_at = Some(std::time::Instant::now());
        self.last_game_at_wall = Some(now_sec());

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
        // The set is durable in sets/*.json now; the journal only ever
        // existed to survive a crash before that happened. Remove it so a
        // stale copy can't be resumed on a later restart (requirement: a
        // finalized set's journal must not be resurrectable).
        self.remove_journal();
        self.emit();
    }

    pub fn idle_check(&mut self, idle_s: f64) {
        // Monotonic elapsed time, not a wall-clock delta - see the comment
        // on `last_game_at`.
        let idle_elapsed = self
            .last_game_at
            .is_some_and(|t| t.elapsed().as_secs_f64() > idle_s);
        if self.set.is_some() && idle_elapsed {
            (self.log)("idle timeout - finalizing open set");
            self.finalize(true, None);
        }
    }

    /// True while a set is open with at least one game in it.
    pub fn has_open_set(&self) -> bool {
        self.set.as_ref().is_some_and(|s| !s.matches.is_empty())
    }

    fn journal_path(&self) -> PathBuf {
        self.out.join(JOURNAL_FILE)
    }

    /// Delete the journal file, if any. Used once a set is durable on disk
    /// (`finalize`) and defensively whenever there's nothing left worth
    /// journaling. Never errors on a missing file - that's the common case.
    fn remove_journal(&self) {
        let _ = std::fs::remove_file(self.journal_path());
    }

    /// Persist the open set (plus whatever `pending` JSON the caller already
    /// has, if any - only `StatsProducer` knows about pending games) so a
    /// crash or restart can resume it. Uses the same atomic tmp+rename as
    /// every other output file here (`write`), so a crash mid-write can't
    /// leave a corrupt journal on disk. If there's nothing open and nothing
    /// pending, the journal is removed instead of writing an empty one.
    fn write_journal(&self, pending: Option<&Value>) {
        match &self.set {
            Some(set) => {
                let journal = json!({
                    "savedAtWall": now_sec(),
                    "openSet": open_set_to_json(set, self.last_game_at_wall),
                    "pending": pending,
                });
                self.write(Path::new(JOURNAL_FILE), &journal);
            }
            None => match pending {
                Some(p) => {
                    // No open set, but a game is still waiting on its replay
                    // (e.g. right after a graceful shutdown raced the replay
                    // showing up) - keep the journal so that isn't lost either.
                    let journal = json!({
                        "savedAtWall": now_sec(),
                        "openSet": Value::Null,
                        "pending": p,
                    });
                    self.write(Path::new(JOURNAL_FILE), &journal);
                }
                None => self.remove_journal(),
            },
        }
    }

    /// Look for a journal left by a previous run and either resume it or,
    /// if it's too stale to plausibly be the same set, finalize it as a
    /// completed set (the games really happened) and log which happened. A
    /// missing, corrupt, or unparseable journal never prevents startup: it
    /// just leaves nothing to resume, same as a fresh install.
    fn resume_journal(&mut self, idle_s: f64) -> JournalResume {
        let none = JournalResume { pending: None };
        let path = self.journal_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return none;
        };
        let parsed: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                (self.log)(&format!(
                    "journal at {} is corrupt ({e}), ignoring it and starting clean",
                    path.display()
                ));
                let _ = std::fs::remove_file(&path);
                return none;
            }
        };
        let Some(saved_at_wall) = parsed["savedAtWall"].as_i64() else {
            (self.log)(&format!(
                "journal at {} is missing its timestamp, ignoring it and starting clean",
                path.display()
            ));
            let _ = std::fs::remove_file(&path);
            return none;
        };

        let now_wall = now_sec();
        let elapsed = (now_wall - saved_at_wall).max(0);
        // `idle_s` is already this app's own definition of "this set is
        // over" (see config.rs's `idle` field doc), so it doubles as the
        // staleness bound here rather than inventing a second timeout. A
        // journal older than that can't plausibly still be the set in
        // progress - resuming it anyway would glue whatever games are
        // played next onto a set that, by the app's own idle rule, already
        // ended (possibly hours ago). A journal within the bound plausibly
        // is still the same set, so it's resumed instead of dropped.
        let stale = (elapsed as f64) > idle_s;

        let open_set_val = parsed.get("openSet").filter(|v| !v.is_null());
        let pending_val = parsed.get("pending").filter(|v| !v.is_null()).cloned();

        if stale {
            let mut finalized_games = 0;
            match open_set_val.map(open_set_from_json) {
                Some(Some((set, _))) if !set.matches.is_empty() => {
                    finalized_games = set.matches.len();
                    self.set = Some(set);
                    // The games in it genuinely happened; only "is this
                    // still the live set" is in question, so they're
                    // finalized as a completed set rather than discarded.
                    // `finalize` also removes the journal file.
                    self.finalize(true, Some(saved_at_wall));
                }
                Some(None) => {
                    (self.log)("stale journal's open-set data was corrupt, discarding it");
                    self.remove_journal();
                }
                _ => self.remove_journal(),
            }
            (self.log)(&format!(
                "journal at {} is {elapsed}s old (idle bound {idle_s}s): {}",
                path.display(),
                if finalized_games > 0 {
                    format!("finalized {finalized_games} recovered game(s) as a completed set")
                } else {
                    "nothing worth recovering, discarded".to_string()
                }
            ));
            // A stale pending game is dropped along with the rest of the
            // journal: PENDING_TIMEOUT_S (90s) is already far shorter than
            // the idle bound used above (420s default), so any pending game
            // old enough to show up in a stale journal has already long
            // since passed its own timeout anyway.
            return JournalResume { pending: None };
        }

        let mut resumed_games = 0;
        if let Some(osv) = open_set_val {
            match open_set_from_json(osv) {
                Some((set, last_wall)) => {
                    resumed_games = set.matches.len();
                    let wall = last_wall.unwrap_or(saved_at_wall);
                    let elapsed_since = (now_wall - wall).max(0) as u64;
                    // Reconstitute the monotonic idle timer from the
                    // wall-clock gap since the last game: `Instant`s from
                    // the previous process are meaningless here, but "how
                    // long ago" survives the restart via wall clock (same
                    // trick `pending_game_from_json` uses for a pending
                    // game's `detected_at` below).
                    self.last_game_at = Some(
                        std::time::Instant::now()
                            .checked_sub(std::time::Duration::from_secs(elapsed_since))
                            .unwrap_or_else(std::time::Instant::now),
                    );
                    self.last_game_at_wall = Some(wall);
                    self.set = Some(set);
                }
                None => (self.log)("resumed journal's open-set data was corrupt, ignoring it"),
            }
        }
        if resumed_games > 0 || pending_val.is_some() {
            (self.log)(&format!(
                "resumed journal ({elapsed}s old): {resumed_games} game(s) in the open set{}",
                if pending_val.is_some() {
                    " plus a pending game"
                } else {
                    ""
                }
            ));
        }
        JournalResume {
            pending: pending_val,
        }
    }
}

/// A game whose save-diff has already been detected but which cannot be
/// recorded yet because its replay isn't on disk (see the race documented on
/// `poll()`). Held until the replay shows up or `PENDING_TIMEOUT_S` elapses.
struct PendingGame {
    result: GameResult,
    /// The (wall-clock) epoch the game was detected at - used both as the
    /// replay-lookup anchor (replay filenames encode real local wall-clock
    /// time, so this has to be real wall-clock time too to compare against
    /// them) and, if nothing else, the timestamp it's eventually recorded
    /// under. Left exactly as wall-clock for both those reasons; see
    /// `detected_at` for the timeout math this used to also be used for.
    at: i64,
    /// Monotonic capture of the same moment, used ONLY by the
    /// `PENDING_TIMEOUT_S` safety-valve check in `resolve_pending`. Kept
    /// separate from `at` rather than replacing it: `at` still has to be a
    /// real wall-clock epoch for replay matching and for the recorded
    /// timestamp above, but the "has this been pending too long" question
    /// must not be corrupted by an NTP step or manual clock change while a
    /// game is in flight (that could force it through early, or never).
    detected_at: std::time::Instant,
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
        let mut machine = SetMachine::new(out_dir, log, on_change)?;
        // Recover (or finalize) whatever a previous run left journaled
        // before anything else touches `machine` - see `resume_journal` for
        // the staleness rule and `set_machine`'s module doc for why this
        // exists at all.
        let resume = machine.resume_journal(idle_s);
        let pending = resume.pending.as_ref().and_then(|v| {
            let p = pending_game_from_json(v, now_sec());
            if p.is_none() {
                (machine.log)("resumed journal's pending-game data was corrupt, ignoring it");
            }
            p
        });
        let mut p = Self {
            save: save_path.to_path_buf(),
            replays: replays_dir.to_path_buf(),
            idle_s,
            log_armed: false,
            machine,
            mtime: None,
            baseline: None,
            pending,
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
                "no replay matched game at {} within {}s, recording without opponent/GameN (unmatched)",
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
        // Monotonic elapsed time, not a wall-clock delta - see the comment
        // on `PendingGame::detected_at`.
        let timed_out = pending.detected_at.elapsed()
            > std::time::Duration::from_secs(Self::PENDING_TIMEOUT_S as u64);
        if !self.try_record_pending(&pending, timed_out) {
            self.pending = Some(pending); // still waiting
        }
        self.write_journal();
    }

    /// Re-journal the current open set + pending game (if any) after
    /// anything that could have changed either. Cheap (a small JSON file,
    /// same atomic tmp+rename as every other output here), so it's simplest
    /// to call it after every mutation point rather than diff against what
    /// was last written.
    fn write_journal(&self) {
        let pending = self.pending.as_ref().map(pending_game_to_json);
        self.machine.write_journal(pending.as_ref());
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
            self.write_journal();
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
        self.pending = Some(PendingGame {
            result,
            at,
            detected_at: std::time::Instant::now(),
        });
        // Try to resolve the fresh pending game immediately in case its
        // replay is already sitting on disk (e.g. after a slow poll tick).
        self.resolve_pending();
    }

    pub fn shutdown(&mut self) {
        self.machine.finalize(false, None);
        // `finalize` already removed the journal for the set that just
        // closed; this re-journals anything it doesn't know about - a game
        // still waiting on its replay, which a graceful shutdown does not
        // otherwise wait for.
        self.write_journal();
    }
}

// ---- Journal (crash/restart durability) ------------------------------------
//
// The open set and any pending game are journaled to `open-set.json` in the
// output dir (see `SetMachine::write_journal` / `resume_journal` above)
// whenever either changes, so a crash or restart mid-set resumes instead of
// silently losing every game played so far. Everything below hand-converts
// the private structs above to/from `serde_json::Value`, matching this
// crate's "typed structs only at the edges" design (see the module doc in
// `lib.rs`) rather than deriving Serialize/Deserialize on `GameResult` et al.

const JOURNAL_FILE: &str = "open-set.json";

/// What `resume_journal` found. `StatsProducer::new` uses `pending` to turn
/// it back into a real `PendingGame` (only `StatsProducer` knows about
/// pending games; `SetMachine` doesn't). `resume_journal` logs the resumed
/// or finalized game counts itself, so they don't need to travel any further
/// than that.
struct JournalResume {
    /// Pending-game JSON recovered from a resumed (non-stale) journal, still
    /// undecoded. A stale journal never yields a pending game (see the
    /// comment in `resume_journal`), so this is only ever set alongside a
    /// non-stale result.
    pending: Option<Value>,
}

fn side_player_to_json(p: &SidePlayer) -> Value {
    json!({"tag": p.tag, "charCode": p.char_code, "mode": p.mode, "stats": p.stats})
}

fn side_player_from_json(v: &Value) -> Option<SidePlayer> {
    Some(SidePlayer {
        tag: v["tag"].as_str()?.to_string(),
        char_code: v["charCode"].as_str()?.to_string(),
        mode: v["mode"].as_str()?.to_string(),
        stats: serde_json::from_value::<BTreeMap<String, i64>>(v["stats"].clone()).ok()?,
    })
}

fn game_result_to_json(r: &GameResult) -> Value {
    json!({
        "mode": r.mode,
        "winners": r.winners.iter().map(side_player_to_json).collect::<Vec<_>>(),
        "losers": r.losers.iter().map(side_player_to_json).collect::<Vec<_>>(),
    })
}

fn game_result_from_json(v: &Value) -> Option<GameResult> {
    let mode = v["mode"].as_str()?.to_string();
    let winners = v["winners"]
        .as_array()?
        .iter()
        .map(side_player_from_json)
        .collect::<Option<Vec<_>>>()?;
    let losers = v["losers"]
        .as_array()?
        .iter()
        .map(side_player_from_json)
        .collect::<Option<Vec<_>>>()?;
    Some(GameResult {
        mode,
        winners,
        losers,
    })
}

/// `last_game_at_wall` rides along with the open set's own fields (rather
/// than as a separate top-level journal key) because the two are only ever
/// meaningful together - there's no "last game recorded at" without an open
/// set to have recorded a game into.
fn open_set_to_json(s: &OpenSet, last_game_at_wall: Option<i64>) -> Value {
    json!({
        "id": s.id, "startEpoch": s.start_epoch, "startIso": s.start_iso,
        "firstMatchStartIso": s.first_match_start_iso, "matches": s.matches,
        "winsByName": s.wins_by_name, "mode": s.mode,
        "lastGameAtWall": last_game_at_wall,
    })
}

fn open_set_from_json(v: &Value) -> Option<(OpenSet, Option<i64>)> {
    let set = OpenSet {
        id: v["id"].as_str()?.to_string(),
        start_epoch: v["startEpoch"].as_i64()?,
        start_iso: v["startIso"].as_str()?.to_string(),
        first_match_start_iso: v["firstMatchStartIso"].as_str().map(|s| s.to_string()),
        matches: v["matches"].as_array()?.clone(),
        wins_by_name: serde_json::from_value(v["winsByName"].clone()).ok()?,
        mode: v["mode"].as_str().map(|s| s.to_string()),
    };
    Some((set, v["lastGameAtWall"].as_i64()))
}

/// `PendingGame::detected_at` (an `Instant`) is never journaled directly - it
/// has no meaning across a process restart. `at` already captures the same
/// moment in wall-clock form (see the doc comment on `detected_at`), so on
/// resume `detected_at` is rebuilt from `at` alone, the same way
/// `resume_journal` rebuilds `last_game_at` from `last_game_at_wall`.
fn pending_game_to_json(p: &PendingGame) -> Value {
    json!({"result": game_result_to_json(&p.result), "at": p.at})
}

fn pending_game_from_json(v: &Value, now_wall: i64) -> Option<PendingGame> {
    let result = game_result_from_json(&v["result"])?;
    let at = v["at"].as_i64()?;
    let elapsed = (now_wall - at).max(0) as u64;
    let detected_at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(elapsed))
        .unwrap_or_else(std::time::Instant::now);
    Some(PendingGame {
        result,
        at,
        detected_at,
    })
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
            detected_at: std::time::Instant::now(),
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
            detected_at: std::time::Instant::now(),
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
        p.pending = Some(PendingGame {
            result: r,
            at,
            detected_at: std::time::Instant::now()
                - std::time::Duration::from_secs(StatsProducer::PENDING_TIMEOUT_S as u64 + 1),
        });

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
        p.machine.last_game_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(10)); // well past idle_s

        // A second game's diff just landed but its replay hasn't yet.
        let r2 = to_game_result(diff(&Map::new(), &g1)).unwrap();
        p.pending = Some(PendingGame {
            result: r2,
            at: now_sec(),
            detected_at: std::time::Instant::now(),
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
            detected_at: std::time::Instant::now(),
        });
        // Directly exercises poll()'s "flush the old pending game first"
        // branch by taking the existing pending game and forcing it through
        // exactly as poll() would when a fresh result arrives.
        let prev = p.pending.take().unwrap();
        p.try_record_pending(&prev, true);
        p.pending = Some(PendingGame {
            result: r2,
            at: now_sec(),
            detected_at: std::time::Instant::now(),
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

    /// A clock that has stepped forward (NTP resync, manual correction) must
    /// not force a pending game through early: `at` (wall clock) looks like
    /// it happened long ago, but `detected_at` (monotonic, injected here to
    /// stand in for "the clock jumped after this was captured") shows only
    /// an instant has really elapsed, so the safety valve must stay quiet.
    #[test]
    fn pending_timeout_ignores_a_forward_wall_clock_jump() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend6_{}", std::process::id()));
        let replays = dir.join("replays"); // left empty - replay never lands
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 180.0);
        p.pending = Some(PendingGame {
            result: r,
            // Wall clock says this happened ages ago (as if the system
            // clock had jumped forward well past PENDING_TIMEOUT_S since).
            at: now_sec() - StatsProducer::PENDING_TIMEOUT_S * 100,
            // But real elapsed time (monotonic) is "just now".
            detected_at: std::time::Instant::now(),
        });

        p.poll();

        assert!(
            p.pending.is_some(),
            "monotonic elapsed time is tiny, so the timeout must not have fired \
             even though the wall-clock delta alone would say otherwise"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clock that has stepped backward must not hide a real timeout: `at`
    /// (wall clock) looks like it's in the future relative to "now" (as a
    /// naive `now_sec() - at` would go negative and never exceed the
    /// timeout), but `detected_at` (monotonic) correctly shows the game has
    /// really been pending longer than PENDING_TIMEOUT_S, so it must still
    /// be forced through rather than held forever.
    #[test]
    fn pending_timeout_still_fires_after_a_backward_wall_clock_jump() {
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();

        let dir = std::env::temp_dir().join(format!("statstest_pend7_{}", std::process::id()));
        let replays = dir.join("replays"); // left empty - replay never lands
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 180.0);
        p.pending = Some(PendingGame {
            result: r,
            // Wall clock looks like it's in the future (as if the system
            // clock had jumped backward after this was captured) - a naive
            // `now_sec() - at` here is negative and would never time out.
            at: now_sec() + 10_000,
            // But real elapsed time (monotonic) is genuinely well past
            // PENDING_TIMEOUT_S.
            detected_at: std::time::Instant::now()
                - std::time::Duration::from_secs(StatsProducer::PENDING_TIMEOUT_S as u64 + 1),
        });

        p.poll();

        assert!(
            p.pending.is_none(),
            "monotonic elapsed time genuinely exceeds the timeout, so it must \
             fire despite the wall clock appearing to be in the future"
        );
        assert!(p.machine.has_open_set(), "the game was still recorded");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same wall-clock-jump immunity, for the idle timer: `last_game_at` is
    /// monotonic, so it cannot be fooled by feeding it a stale-looking
    /// instant the way the old `now_sec()`-based version could be by an
    /// actual clock jump.
    #[test]
    fn idle_check_uses_monotonic_elapsed_time() {
        let dir = std::env::temp_dir().join(format!("statstest_idle_mono_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let rep = parse_replay_name("2026-07-23_19-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();
        m.record_game(&r, Some(&rep), rep.epoch);

        // idle_s generous enough that the file writes record_game() just did
        // can't possibly have eaten it, but the deliberate backward-shifted
        // check below is still comfortably past it; a fresh Instant (elapsed
        // well under a second) must NOT exceed this.
        m.idle_check(5.0);
        assert!(
            m.has_open_set(),
            "an Instant captured moments ago should not read as idle-elapsed yet"
        );

        // Now push last_game_at genuinely backward via the monotonic clock
        // (not a wall-clock trick) and confirm idle_check does fire once
        // real elapsed time exceeds idle_s.
        m.last_game_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        m.idle_check(0.001);
        assert!(
            !m.has_open_set(),
            "idle_check should finalize once real elapsed time exceeds idle_s"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- Journal (crash/restart durability) --------------------------------

    /// A game recorded, journaled, then read back by a fresh `SetMachine`
    /// (standing in for a process restart) should come back with the same
    /// open set - the basic round trip the rest of the durability behaviour
    /// builds on.
    #[test]
    fn journal_round_trips_an_open_set() {
        let dir = std::env::temp_dir().join(format!("statstest_journal_rt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let rep = parse_replay_name("2026-07-23_19-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();

        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        m.record_game(&r, Some(&rep), rep.epoch);
        m.write_journal(None); // no pending game in this scenario
        assert!(m.journal_path().is_file());

        // A fresh SetMachine over the same out dir, as if the process had
        // just restarted.
        let mut m2 = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        let resume = m2.resume_journal(180.0); // generous idle bound; this journal is seconds old
        assert!(resume.pending.is_none());
        assert!(m2.has_open_set(), "the open set should have been resumed");
        assert_eq!(m2.set.as_ref().unwrap().matches.len(), 1);
        assert_eq!(
            m2.set.as_ref().unwrap().id,
            m.set.as_ref().unwrap().id,
            "the resumed set should be the same set, not a fresh one"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A journal within the staleness bound resumes with its monotonic
    /// timers correctly reconstituted from the wall clock: `last_game_at`
    /// must reflect the real elapsed time since the last game (here, ~60s),
    /// not zero (which would fail to idle-finalize when it should) and not
    /// some inflated value (which would finalize immediately when it
    /// shouldn't).
    #[test]
    fn resume_reconstitutes_the_monotonic_idle_timer_from_wall_clock() {
        let dir =
            std::env::temp_dir().join(format!("statstest_journal_resume_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A journal as if the process had been killed 60s ago mid-set.
        let past = now_sec() - 60;
        let open_set = OpenSet {
            id: "20260723_190000".into(),
            start_epoch: past,
            start_iso: iso_of(past),
            first_match_start_iso: Some(iso_of(past)),
            matches: vec![json!({"index": 1, "players": []})],
            wins_by_name: Map::new(),
            mode: Some("LOCAL".into()),
        };
        let journal = json!({
            "savedAtWall": past,
            "openSet": open_set_to_json(&open_set, Some(past)),
            "pending": Value::Null,
        });
        std::fs::write(
            dir.join("open-set.json"),
            serde_json::to_string_pretty(&journal).unwrap(),
        )
        .unwrap();

        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        let resume = m.resume_journal(420.0); // default idle bound; 60s old is well inside
        assert!(resume.pending.is_none());
        assert!(
            m.has_open_set(),
            "a 60s-old journal within a 420s idle bound should resume, not finalize"
        );

        let elapsed = m.last_game_at.unwrap().elapsed().as_secs_f64();
        assert!(
            (59.0..90.0).contains(&elapsed),
            "reconstituted elapsed time should be ~60s, was {elapsed}"
        );

        // Proof the 60s really carried across the restart (not reset to
        // "just now"): idle_check with idle_s below that must fire now,
        // where it would not have if last_game_at had come back as fresh.
        m.idle_check(30.0);
        assert!(
            !m.has_open_set(),
            "idle_check should finalize once the reconstituted elapsed time exceeds idle_s"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A journal from long before the idle bound (hours, not seconds) must
    /// not be resumed as the live set - that would glue unrelated future
    /// games onto a set that, by the app's own idle rule, ended long ago.
    /// Its games did happen, though, so they're finalized as a completed set
    /// instead of just being dropped.
    #[test]
    fn stale_journal_is_finalized_as_a_completed_set_not_resumed() {
        let dir =
            std::env::temp_dir().join(format!("statstest_journal_stale_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let past = now_sec() - 3 * 3600; // three hours ago
        let open_set = OpenSet {
            id: "20260723_120000".into(),
            start_epoch: past,
            start_iso: iso_of(past),
            first_match_start_iso: Some(iso_of(past)),
            matches: vec![json!({"index": 1, "players": []})],
            wins_by_name: Map::new(),
            mode: Some("LOCAL".into()),
        };
        let journal = json!({
            "savedAtWall": past,
            "openSet": open_set_to_json(&open_set, Some(past)),
            "pending": Value::Null,
        });
        std::fs::write(
            dir.join("open-set.json"),
            serde_json::to_string_pretty(&journal).unwrap(),
        )
        .unwrap();

        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        let resume = m.resume_journal(420.0); // idle bound (7 min) far shorter than 3h
        assert!(resume.pending.is_none());
        assert!(
            !m.has_open_set(),
            "a 3-hour-old journal must not be resumed as the live set"
        );

        let files: Vec<_> = std::fs::read_dir(dir.join("sets"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(
            files.len(),
            1,
            "the stale journal's games should be finalized to disk instead of dropped"
        );

        assert!(
            !m.journal_path().is_file(),
            "the stale journal must be removed so it can't be resurrected on a later restart"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt/truncated/unparseable journal must never prevent startup -
    /// exercised through the real startup path (`StatsProducer::new`), which
    /// is what actually runs on app launch.
    #[test]
    fn corrupt_journal_does_not_prevent_startup() {
        let dir =
            std::env::temp_dir().join(format!("statstest_journal_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("open-set.json"), b"{ this is not valid json [[[").unwrap();

        let p = StatsProducer::new(
            &dir.join("missing.sav"),
            &dir.join("replays"),
            &dir,
            420.0,
            Box::new(|_| {}),
            None,
        );
        assert!(p.is_ok(), "a corrupt journal must never prevent startup");
        let p = p.unwrap();
        assert!(!p.machine.has_open_set());
        assert!(
            !dir.join("open-set.json").is_file(),
            "the corrupt journal should be cleaned up rather than left to fail again next time"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finalizing a set normally (not via a stale-journal recovery) removes
    /// the journal - a later restart must not find and resurrect it.
    #[test]
    fn journal_removed_on_normal_finalize() {
        let dir =
            std::env::temp_dir().join(format!("statstest_journal_cleanup_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let g1 = snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("losses", 1.0)]),
        ]);
        let r = to_game_result(diff(&Map::new(), &g1)).unwrap();
        let rep = parse_replay_name("2026-07-23_19-00-00-000-A(Cla)-B(Kra)-Game1.rpl").unwrap();

        let mut m = SetMachine::new(&dir, Box::new(|_| {}), None).unwrap();
        m.record_game(&r, Some(&rep), rep.epoch);
        m.write_journal(None);
        assert!(
            m.journal_path().is_file(),
            "sanity check: journal exists before finalize"
        );

        m.finalize(true, Some(rep.epoch + 5));
        assert!(
            !m.journal_path().is_file(),
            "finalize must remove the journal"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pending game (detected, but still waiting on its replay) survives a
    /// restart too, via the same journal - and its monotonic
    /// `PENDING_TIMEOUT_S` timer is correctly reconstituted from wall clock,
    /// so it doesn't immediately get forced through as unmatched right after
    /// resuming.
    #[test]
    fn pending_game_survives_the_journal_round_trip() {
        let g_online = snap(&[("ME", "Fle", "ONLINE", &[("wins", 1.0)])]);
        let r = to_game_result(diff(&Map::new(), &g_online)).unwrap();

        let dir =
            std::env::temp_dir().join(format!("statstest_journal_pending_{}", std::process::id()));
        let replays = dir.join("replays");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&replays).unwrap();

        let mut p = producer(&dir, &replays, 420.0);
        p.pending = Some(PendingGame {
            result: r,
            at: now_sec(),
            detected_at: std::time::Instant::now(),
        });
        p.write_journal();
        assert!(p.machine.journal_path().is_file());

        // A brand-new StatsProducer over the same out dir, as if the process
        // had just restarted.
        let p2 = producer(&dir, &replays, 420.0);
        assert!(
            p2.pending.is_some(),
            "the pending game should have been resumed from the journal"
        );
        let pending = p2.pending.as_ref().unwrap();
        assert_eq!(pending.result.winners[0].tag, "ME");
        assert_eq!(pending.at, p.pending.as_ref().unwrap().at);
        // Reconstituted from "just now", not from zero and not already past
        // PENDING_TIMEOUT_S - either would make it resolve incorrectly on
        // the very next poll.
        assert!(
            pending.detected_at.elapsed().as_secs_f64() < 5.0,
            "detected_at should reconstitute to ~now, not drift or reset oddly"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
