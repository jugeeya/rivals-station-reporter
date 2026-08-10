//! The engine thread — the Rust equivalent of the Python widget's `_loop`.
//!
//! Owns the stats producer (station), the forwarder (station), and — in
//! operator mode — the LAN hub. The UI is a pure subscriber: the engine
//! pushes a full state snapshot through the emitter callback (the Iced app
//! feeds it into a Subscription channel) and `state()` answers the initial
//! read with the same JSON. Ported from the Tauri shell's engine.rs; the
//! only change is this decoupling — Tauri's `AppHandle`/`Emitter` became
//! `set_emitter`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::{json, Value};

use station_core::forwarder::Forwarder;
use station_core::matching;
use station_core::save::{check_replay_autosave, ReplayAutoSaveStatus};
use station_core::set_machine::StatsProducer;
use station_core::tagdb::TagDb;

use crate::engine::config::{self, Config};

/// Wording shared by the "Replay Auto Save is misconfigured" warning — used
/// both for the persistent status override and the one-time log line so the
/// two never drift apart.
fn replay_autosave_warning(raw: &str) -> String {
    format!(
        "Replay Auto Save is set to '{raw}' in Rivals 2 — set it to All or Local Only \
         (Options > Gameplay) or this app can't record any sets"
    )
}

/// Wording shared by the "station clock disagrees with the hub" warning -
/// same reasoning as `replay_autosave_warning` above.
fn clock_skew_warning(skew_s: i64) -> String {
    let direction = if skew_s >= 0 { "ahead of" } else { "behind" };
    format!(
        "This station's clock is {}s {direction} the hub's - replay matching and idle/timeout \
         detection will misbehave until the system clock is corrected",
        skew_s.abs()
    )
}

const LOG_LINES: usize = 200;

pub struct Engine(pub Arc<EngineInner>);

pub struct EngineInner {
    /// Where full state snapshots go on every change — the UI's event feed.
    /// Registered after the engine starts (`set_emitter`); emissions before
    /// that are simply dropped, matching the old shell where events fired
    /// before the webview subscribed went nowhere.
    emitter: Mutex<Option<Box<dyn Fn(Value) + Send + Sync>>>,
    pub config_dir: PathBuf,
    pub cfg: Mutex<Config>,
    rebuild: AtomicBool,
    /// Pokes the loop thread awake so a config save applies NOW instead of
    /// after the current poll sleep runs out (the wake is the sender half;
    /// the loop parks in `recv_timeout` on the other end).
    rebuild_wake: Mutex<std::sync::mpsc::Sender<()>>,
    status: Mutex<Value>, // {msg, error, t}
    log: Mutex<VecDeque<String>>,
    snapshot: Mutex<Value>,     // station producer snapshot {history, live}
    hub_snapshot: Mutex<Value>, // operator hub snapshot {sets, stations}
    hub_url: Mutex<Option<String>>,
    // The running hub (operator mode), for the report/swap/delete commands.
    hub: Mutex<Option<Arc<station_core::hub::Hub>>>,
    armed: AtomicBool, // stats save readable at least once

    // The public tag database (jugeeya.github.io/tags): loaded from its
    // on-disk cache at startup (no network — see `TagDb::load`) and kept
    // fresh by its own background thread. Owned here, not by the hub, so a
    // pure `station` install with no hub at all still gets a resolved
    // start.gg handle on the snapshot (see `annotate_sgg` in `state()`);
    // `build_hub` also reads it (via `tagdb_map`) as the lowest-precedence
    // matching layer when operator mode is running.
    tagdb: Arc<TagDb>,

    // "Replay Auto Save" health check (see station_core::save). Tracked
    // separately from `status` above and re-applied as an override in
    // `state()` — see the comment on `check_replay_autosave` for why.
    replay_autosave_mtime: Mutex<Option<SystemTime>>,
    replay_autosave_bad: Mutex<Option<(String, i64)>>, // (raw enum value, first-seen t)

    // Station-clock-vs-hub-clock warning (see `check_clock_skew` and
    // `Forwarder::clock_skew_check`). Same separated-latch shape as
    // `replay_autosave_bad` and for the same reason (status/log_line are
    // clobbered by ordinary operation). Unlike `replay_autosave_bad`'s raw
    // enum value, a measured skew jitters a little tick to tick even while
    // still genuinely bad, so `check_clock_skew` only logs on the "was
    // fine, now bad"/"was bad, now fine" transitions, not on every wobble.
    clock_skew_bad: Mutex<Option<(String, i64)>>, // (message, first-seen t)

    // Hub/broker connectivity warning (see `check_forward_health` and
    // `Forwarder::forward_status`). Same shape and same anti-jitter
    // reasoning as `clock_skew_bad` (the failure kind can flip between
    // calls, e.g. unreachable while the hub restarts, then a bad key once
    // it's back up with different config).
    forward_bad: Mutex<Option<(String, i64)>>, // (message, first-seen t)

    /// Screenshot/dev only (`seed_dev_state`): replaces the computed health
    /// block wholesale so captures don't show this machine's missing game
    /// paths. Never set in normal operation.
    dev_health: Mutex<Option<Value>>,
    /// Same dev-only mechanism for the status line.
    dev_status: Mutex<Option<Value>>,
}

impl EngineInner {
    fn now() -> i64 {
        station_core::now_sec()
    }

    pub fn log_line(&self, msg: &str) {
        let lower = msg.to_lowercase();
        let error = lower.contains("fail") || lower.contains("error") || lower.contains("warning");
        self.set_status(msg, error);
        let stamp = chrono_lite_hms();
        let mut log = self.log.lock().unwrap();
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(format!("{stamp}  {msg}"));
    }

    pub fn set_status(&self, msg: &str, error: bool) {
        *self.status.lock().unwrap() = json!({ "msg": msg, "error": error, "t": Self::now() });
    }

    /// Re-check the "Replay Auto Save" gameplay setting (a settings save
    /// separate from the stats save/replays this app already watches — see
    /// `station_core::save::check_replay_autosave`). Only meaningful in
    /// station mode, since that's the side that reads local replays at all.
    ///
    /// Cheap to call on every engine tick: the settings file's mtime is
    /// checked first and the file is only re-parsed when it changes, same
    /// pattern `StatsProducer::poll` uses for the stats save.
    ///
    /// A confirmed bad value is latched into `replay_autosave_bad` rather
    /// than written through `set_status` directly — `set_status`/`log_line`
    /// are called constantly by ordinary operation (every game recorded,
    /// every settings save, hub events, ...) and any of those would
    /// immediately clobber a one-shot status write. Latching it and having
    /// `state()` re-apply it as a final override on every read means the
    /// warning survives no matter what else touches `status` in between,
    /// until the setting is confirmed fixed.
    pub fn check_replay_autosave(&self) {
        if !self.cfg.lock().unwrap().is_station() {
            // Not reading local replays at all right now — don't leave a
            // stale warning showing from a previous station-mode run. Also
            // forget the last-seen mtime so switching back to station mode
            // always re-parses instead of trusting a stale "unchanged" skip.
            *self.replay_autosave_bad.lock().unwrap() = None;
            *self.replay_autosave_mtime.lock().unwrap() = None;
            return;
        }
        let path = config::default_settings_path();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        {
            let mut last = self.replay_autosave_mtime.lock().unwrap();
            if mtime.is_some() && *last == mtime {
                return; // unchanged since last check; keep current verdict
            }
            *last = mtime;
        }
        match check_replay_autosave(&path) {
            ReplayAutoSaveStatus::BadValue(raw) => {
                let already_flagged = self
                    .replay_autosave_bad
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|(v, _)| v == &raw);
                if !already_flagged {
                    *self.replay_autosave_bad.lock().unwrap() = Some((raw.clone(), Self::now()));
                    self.log_line(&format!("WARNING: {}", replay_autosave_warning(&raw)));
                }
            }
            ReplayAutoSaveStatus::Ok | ReplayAutoSaveStatus::Unknown => {
                let was_bad = self.replay_autosave_bad.lock().unwrap().take().is_some();
                if was_bad {
                    self.log_line("Replay Auto Save looks correctly configured now.");
                }
            }
        }
    }

    /// Re-latch the clock-skew warning from the forwarder's latest probe of
    /// the hub's clock (`Forwarder::clock_skew_check`, called once per loop
    /// tick alongside this). Same separated-latch mechanism as
    /// `check_replay_autosave` and for the same reason.
    ///
    /// `skew_s` is `None` whenever the forwarder has nothing confirmed to
    /// report either way (dry run, no hub reachable, or a hub/broker too old
    /// to answer with its own clock) - that is deliberately treated as "no
    /// news", not "confirmed fine": a transient probe failure must not
    /// silently clear a real warning that's still true. A station with no
    /// hub configured at all never even reaches this method - see the
    /// `else` branch in the engine loop that calls `clear_forward_warnings`
    /// instead when there's no forwarder to ask.
    pub fn check_clock_skew(&self, skew_s: Option<i64>) {
        let Some(skew_s) = skew_s else { return };
        let mut bad = self.clock_skew_bad.lock().unwrap();
        if skew_s.abs() > Forwarder::CLOCK_SKEW_WARN_S {
            let msg = clock_skew_warning(skew_s);
            let was_bad = bad.is_some();
            let first_seen = bad.as_ref().map(|(_, t)| *t).unwrap_or_else(Self::now);
            *bad = Some((msg.clone(), first_seen));
            drop(bad);
            if !was_bad {
                self.log_line(&format!("WARNING: {msg}"));
            }
        } else {
            let was_bad = bad.take().is_some();
            drop(bad);
            if was_bad {
                self.log_line("Station clock is back in sync with the hub.");
            }
        }
    }

    /// Re-latch the hub/broker connectivity warning from the forwarder's
    /// consecutive-failure tracking (`Forwarder::forward_status`). Same
    /// mechanism as `check_clock_skew` above, including "no reading yet"
    /// (`None`, e.g. dry run or too few failures to be sure) leaving any
    /// existing latch untouched rather than clearing it.
    pub fn check_forward_health(&self, status: Option<String>) {
        let mut bad = self.forward_bad.lock().unwrap();
        match status {
            Some(msg) => {
                let was_bad = bad.is_some();
                let first_seen = bad.as_ref().map(|(_, t)| *t).unwrap_or_else(Self::now);
                *bad = Some((msg.clone(), first_seen));
                drop(bad);
                if !was_bad {
                    self.log_line(&format!("WARNING: {msg}"));
                }
            }
            None => {
                let was_bad = bad.take().is_some();
                drop(bad);
                if was_bad {
                    self.log_line("Hub/broker connection recovered.");
                }
            }
        }
    }

    /// Called on every tick where there is no forwarder at all (no
    /// hub/broker configured, or station mode is off): there is nothing to
    /// compare a clock against or fail to reach, so a warning latched under
    /// a previous configuration must not linger forever.
    pub fn clear_forward_warnings(&self) {
        *self.clock_skew_bad.lock().unwrap() = None;
        *self.forward_bad.lock().unwrap() = None;
    }

    /// The one JSON blob the UI renders from.
    pub fn state(&self) -> Value {
        let cfg = self.cfg.lock().unwrap().clone();
        let (def_save, def_replays) = config::default_save_paths();
        let save = if cfg.save.is_empty() {
            def_save
        } else {
            PathBuf::from(&cfg.save)
        };
        let replays = if cfg.replays.is_empty() {
            def_replays
        } else {
            PathBuf::from(&cfg.replays)
        };
        let out_dir = self.out_dir(&cfg);
        // A confirmed-bad health latch overrides whatever the normal status
        // flow last wrote; see `check_replay_autosave` for why this can't
        // just be another `set_status` call. Priority when more than one is
        // active: Replay Auto Save misconfigured means NOTHING is being
        // recorded at all (the most severe failure this app can detect);
        // clock skew corrupts matching/timeouts but games are still being
        // recorded; a forwarder outage means games are still recorded and
        // matched locally, just not yet sent anywhere, so it's shown last.
        let status = if let Some(s) = self.dev_status.lock().unwrap().clone() {
            // Screenshot/dev seed: pinned status, bypassing the health
            // latches (which would otherwise report this machine's real,
            // irrelevant problems into the capture).
            s
        } else {
            match (
                &*self.replay_autosave_bad.lock().unwrap(),
                &*self.clock_skew_bad.lock().unwrap(),
                &*self.forward_bad.lock().unwrap(),
            ) {
                (Some((raw, first_seen_t)), _, _) => json!({
                    "msg": replay_autosave_warning(raw),
                    "error": true,
                    "t": first_seen_t,
                }),
                (_, Some((msg, first_seen_t)), _) => json!({
                    "msg": msg,
                    "error": true,
                    "t": first_seen_t,
                }),
                (_, _, Some((msg, first_seen_t))) => json!({
                    "msg": msg,
                    "error": true,
                    "t": first_seen_t,
                }),
                _ => self.status.lock().unwrap().clone(),
            }
        };
        json!({
            "config": cfg,
            "status": status,
            "snapshot": annotate_sgg(&self.snapshot.lock().unwrap(), &self.tagdb.map()),
            "hubSnapshot": *self.hub_snapshot.lock().unwrap(),
            "hubUrl": *self.hub_url.lock().unwrap(),
            "log": self.log.lock().unwrap().iter().cloned().collect::<Vec<_>>(),
            "health": self.dev_health.lock().unwrap().clone().unwrap_or_else(|| json!({
                "savePath": save.to_string_lossy(),
                "saveExists": save.is_file(),
                "saveArmed": self.armed.load(Ordering::Relaxed),
                "replaysPath": replays.to_string_lossy(),
                "replaysExists": replays.is_dir(),
                "outDir": out_dir.to_string_lossy(),
            })),
        })
    }

    pub fn emit_state(&self) {
        if let Some(emit) = self.emitter.lock().unwrap().as_ref() {
            emit(self.state());
        }
    }

    /// Hook the UI's event feed up. Immediately pushes the current state so
    /// a subscriber that attaches after startup isn't stuck on defaults
    /// until the next engine tick.
    pub fn set_emitter(&self, emit: Box<dyn Fn(Value) + Send + Sync>) {
        emit(self.state());
        *self.emitter.lock().unwrap() = Some(emit);
    }

    pub fn request_rebuild(&self) {
        self.rebuild.store(true, Ordering::SeqCst);
        let _ = self.rebuild_wake.lock().unwrap().send(());
    }

    /// Dev/screenshot hook: overwrite the station snapshot and/or hub
    /// snapshot with fixture data (the native counterpart of the browser
    /// mock's `__RSR_SEED__`). Only ever called when RSR_SEED_STATE is set —
    /// never in normal operation, where the producer/hub own these.
    pub fn seed_dev_state(
        &self,
        snapshot: Option<Value>,
        hub_snapshot: Option<Value>,
        health: Option<Value>,
        status: Option<Value>,
    ) {
        if let Some(s) = snapshot {
            *self.snapshot.lock().unwrap() = s;
        }
        if let Some(h) = hub_snapshot {
            *self.hub_snapshot.lock().unwrap() = h;
        }
        if health.is_some() {
            *self.dev_health.lock().unwrap() = health;
        }
        if status.is_some() {
            *self.dev_status.lock().unwrap() = status;
        }
        self.emit_state();
    }

    pub fn set_hub_snapshot(&self, snap: Value) {
        *self.hub_snapshot.lock().unwrap() = snap;
    }

    pub fn set_hub(&self, hub: Option<Arc<station_core::hub::Hub>>) {
        *self.hub.lock().unwrap() = hub;
    }

    pub fn hub(&self) -> Option<Arc<station_core::hub::Hub>> {
        self.hub.lock().unwrap().clone()
    }

    /// The tag database's current save-tag -> start.gg-handle map. Cheap
    /// (clones an in-memory `HashMap`); never touches the network itself —
    /// see `station_core::tagdb::TagDb`.
    pub fn tagdb_map(&self) -> HashMap<String, String> {
        self.tagdb.map()
    }

    fn out_dir(&self, cfg: &Config) -> PathBuf {
        if cfg.dir.is_empty() {
            self.config_dir.join("matchlogger-out")
        } else {
            PathBuf::from(&cfg.dir)
        }
    }
}

/// Attach the tag database's resolved start.gg handle (if any) to every
/// player in a station snapshot — `sgg: Some(handle)` when the public tag
/// database says this in-game tag belongs to `@handle`, `None` otherwise.
/// Deliberately just that lookup: no fuzzy guessing, and nothing to do with
/// the bracket-derived 2-entrant match the hub does elsewhere.
///
/// A pure function (not an `EngineInner` method) so pure `station` mode —
/// no hub, no start.gg token, no slug — still gets this: the engine always
/// has its own `TagDb`, and every `state()` read re-annotates from whatever
/// map it currently holds, so a background refresh shows up immediately.
fn annotate_sgg(snapshot: &Value, tagdb_map: &HashMap<String, String>) -> Value {
    fn annotate_players(set: &mut Value, tagdb_map: &HashMap<String, String>) {
        let Some(players) = set.get_mut("players").and_then(|p| p.as_array_mut()) else {
            return;
        };
        for p in players {
            let tag = p.get("tag").cloned().unwrap_or(Value::Null);
            let handle = tagdb_map.get(&matching::norm(&tag)).cloned();
            p["sgg"] = handle.map(Value::String).unwrap_or(Value::Null);
        }
    }

    let mut out = snapshot.clone();
    if let Some(history) = out.get_mut("history").and_then(|h| h.as_array_mut()) {
        for set in history {
            annotate_players(set, tagdb_map);
        }
    }
    if let Some(live) = out.get_mut("live") {
        if !live.is_null() {
            annotate_players(live, tagdb_map);
        }
    }
    out
}

fn chrono_lite_hms() -> String {
    // Local wall-clock HH:MM:SS for the log panel, like the Python widget.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // chrono is available via station-core's re-export path; format locally.
    let dt = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let datetime: chrono::DateTime<chrono::Local> = dt.into();
    datetime.format("%H:%M:%S").to_string()
}

/// What one rebuild produced — the live pieces the loop drives.
struct Built {
    producer: Option<StatsProducer>,
    forwarder: Option<Forwarder>,
    hub_pieces: Option<crate::engine::hub_glue::HubPieces>,
}

fn build(inner: &Arc<EngineInner>) -> Built {
    let cfg = inner.cfg.lock().unwrap().clone();

    // Operator side first: the local forwarder points at our own hub in
    // "both" mode (don't round-trip the LAN).
    let hub_pieces = if cfg.is_operator() {
        match crate::engine::hub_glue::build_hub(inner, &cfg) {
            Ok(p) => {
                *inner.hub_url.lock().unwrap() = Some(p.url.clone());
                Some(p)
            }
            Err(e) => {
                inner.log_line(&format!("hub failed to start: {e}"));
                *inner.hub_url.lock().unwrap() = None;
                inner.set_hub(None);
                // Without this the UI keeps rendering the dead hub's last
                // sets/stations as if the console were still live.
                inner.set_hub_snapshot(json!({ "sets": [], "stations": {} }));
                None
            }
        }
    } else {
        *inner.hub_url.lock().unwrap() = None;
        inner.set_hub(None);
        // Same as the Err arm: leaving operator mode must not keep serving
        // the previous operator run's snapshot.
        inner.set_hub_snapshot(json!({ "sets": [], "stations": {} }));
        None
    };

    // Runs once here (rebuild/startup) and again every loop tick below —
    // cheap either way since it only re-parses the settings file when its
    // mtime changes.
    inner.check_replay_autosave();

    let mut producer = None;
    let mut forwarder = None;
    if cfg.is_station() {
        let (def_save, def_replays) = config::default_save_paths();
        let save = if cfg.save.is_empty() {
            def_save
        } else {
            PathBuf::from(&cfg.save)
        };
        let replays = if cfg.replays.is_empty() {
            def_replays
        } else {
            PathBuf::from(&cfg.replays)
        };
        let out_dir = inner.out_dir(&cfg);

        let log_inner = inner.clone();
        let snap_inner = inner.clone();
        match StatsProducer::new(
            &save,
            &replays,
            &out_dir,
            cfg.idle,
            Box::new(move |m| {
                log_inner.log_line(m);
            }),
            Some(Box::new(move |snap| {
                *snap_inner.snapshot.lock().unwrap() = snap.clone();
                snap_inner.emit_state();
            })),
        ) {
            Ok(p) => {
                inner.armed.store(p.armed(), Ordering::Relaxed);
                producer = Some(p);
            }
            Err(e) => inner.log_line(&format!("stats setup error: {e}")),
        }

        // Forwarding is OPTIONAL — with no broker/slug it runs local-only
        // (scoreboard + files), so it works even without a bracket.
        let broker = match &hub_pieces {
            // "both": talk to our own hub over loopback.
            Some(p) => format!("http://127.0.0.1:{}", p.port),
            None => cfg.broker.clone(),
        };
        if !broker.is_empty() && !cfg.slug.is_empty() {
            let log_inner = inner.clone();
            forwarder = Some(Forwarder::new(
                &broker,
                &cfg.slug,
                cfg.station,
                &inner.out_dir(&cfg),
                None,
                cfg.dry_run,
                if cfg.key.is_empty() {
                    None
                } else {
                    Some(&cfg.key)
                },
                Box::new(move |m| log_inner.log_line(m)),
            ));
        } else {
            inner.log_line("local-only: no event/broker configured, nothing is sent");
        }
    }

    Built {
        producer,
        forwarder,
        hub_pieces,
    }
}

/// Create the engine and start its loop thread.
pub fn start(config_dir: PathBuf) -> Engine {
    let cfg = config::load(&config_dir);
    // Disk-only, never the network (see `TagDb::load`) — safe to run inline
    // here rather than deferring it into the loop thread below.
    let tagdb = TagDb::load(&config_dir);
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let inner = Arc::new(EngineInner {
        emitter: Mutex::new(None),
        config_dir,
        cfg: Mutex::new(cfg),
        rebuild: AtomicBool::new(true),
        rebuild_wake: Mutex::new(wake_tx),
        status: Mutex::new(
            json!({ "msg": "starting…", "error": false, "t": station_core::now_sec() }),
        ),
        log: Mutex::new(VecDeque::new()),
        snapshot: Mutex::new(json!({ "history": [], "live": null })),
        hub_snapshot: Mutex::new(json!({ "sets": [], "stations": {} })),
        hub_url: Mutex::new(None),
        hub: Mutex::new(None),
        armed: AtomicBool::new(false),
        tagdb,
        replay_autosave_mtime: Mutex::new(None),
        replay_autosave_bad: Mutex::new(None),
        clock_skew_bad: Mutex::new(None),
        forward_bad: Mutex::new(None),
        dev_health: Mutex::new(None),
        dev_status: Mutex::new(None),
    });

    // Background refresher — its own thread, its own schedule, never the
    // engine loop below (see `TagDb::spawn_refresh`).
    let tagdb_log = inner.clone();
    inner
        .tagdb
        .spawn_refresh(Box::new(move |m| tagdb_log.log_line(m)));

    let loop_inner = inner.clone();
    std::thread::spawn(move || {
        let mut built = Built {
            producer: None,
            forwarder: None,
            hub_pieces: None,
        };
        loop {
            if loop_inner.rebuild.swap(false, Ordering::SeqCst) {
                // Drop the old pieces first so ports/files release.
                if let Some(p) = built.hub_pieces.take() {
                    loop_inner.set_hub(None);
                    p.stop();
                }
                built = build(&loop_inner);
                loop_inner.emit_state();
            }
            if let Some(p) = &mut built.producer {
                p.poll();
                loop_inner.armed.store(p.armed(), Ordering::Relaxed);
            }
            if let Some(f) = &mut built.forwarder {
                f.tick();
                // `forward_status` is free (in-memory counters); the clock
                // probe throttles its own network hit internally (see
                // `Forwarder::clock_skew_check`), so both are cheap to call
                // every tick.
                loop_inner.check_forward_health(f.forward_status());
                loop_inner.check_clock_skew(f.clock_skew_check());
            } else {
                // No hub/broker configured at all (or station mode is off)
                // - nothing to compare a clock against or fail to reach.
                loop_inner.clear_forward_warnings();
            }
            // mtime-gated, so this is a no-op stat() call on almost every
            // tick — only re-parses the settings file when it changes.
            loop_inner.check_replay_autosave();
            loop_inner.emit_state();
            // Bounded on BOTH sides, via max/min rather than `clamp` (which
            // passes NaN through): `from_secs_f64` PANICS on a huge or
            // non-finite value, and a hand-edited config.json (`"poll": 1e300`)
            // must not be able to kill this thread — the window would stay up
            // while watching/forwarding silently stopped forever.
            let poll = loop_inner.cfg.lock().unwrap().poll.max(0.5).min(60.0);
            // Parks for one poll interval OR until request_rebuild pokes the
            // wake channel — a settings save takes effect in milliseconds
            // instead of "whenever the sleep ends". A closed channel can't
            // happen (the sender lives inside EngineInner), but if it ever
            // did, timing out is the correct degraded behavior anyway.
            let _ = wake_rx.recv_timeout(std::time::Duration::from_secs_f64(poll));
        }
    });

    Engine(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player known to the tag database gets `sgg` set to the resolved
    /// start.gg handle; one it doesn't know gets `sgg: null` — never a
    /// fuzzy guess, never left off the object entirely.
    #[test]
    fn annotate_sgg_resolves_known_tags_and_nulls_unknown_ones() {
        let snapshot = json!({
            "history": [{
                "startEpoch": 1, "complete": true, "mode": "LOCAL", "games": 2,
                "players": [
                    {"tag": "JUGZ!", "char": "Orcane", "wins": 2, "slot": 0, "won": true},
                    {"tag": "NOBODY", "char": "Galvan", "wins": 0, "slot": 1, "won": false},
                ],
            }],
            "live": {
                "startEpoch": 2, "complete": false, "mode": "LOCAL", "games": 1,
                "players": [
                    {"tag": "jugz", "char": "Orcane", "wins": 1, "slot": 0, "won": false},
                ],
            },
        });
        let tagdb_map: HashMap<String, String> = [("jugz".to_string(), "jugeeya".to_string())]
            .into_iter()
            .collect();

        let out = annotate_sgg(&snapshot, &tagdb_map);

        let hist_players = out["history"][0]["players"].as_array().unwrap();
        assert_eq!(
            hist_players[0]["sgg"],
            json!("jugeeya"),
            "a tag the database knows resolves to its start.gg handle"
        );
        assert_eq!(
            hist_players[1]["sgg"],
            Value::Null,
            "a tag the database doesn't know is null, not a guess"
        );
        // The tag database normalizes the same way `matching::norm` does, so
        // "jugz" (lowercase, live set) still matches the "JUGZ!" entry.
        assert_eq!(
            out["live"]["players"][0]["sgg"],
            json!("jugeeya"),
            "matching is case/punctuation-insensitive, same as the rest of matching.rs"
        );
    }

    /// A `null` live set (no open set) must pass through untouched rather
    /// than panicking — pure station mode with nothing running yet.
    #[test]
    fn annotate_sgg_tolerates_a_null_live_set() {
        let snapshot = json!({ "history": [], "live": null });
        let out = annotate_sgg(&snapshot, &HashMap::new());
        assert_eq!(out["live"], Value::Null);
        assert_eq!(out["history"], json!([]));
    }
}
