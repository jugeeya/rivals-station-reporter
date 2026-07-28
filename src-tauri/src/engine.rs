//! The engine thread — the Rust equivalent of the Python widget's `_loop`.
//!
//! Owns the stats producer (station), the forwarder (station), and — in
//! operator mode — the LAN hub. The UI is a pure subscriber: the engine
//! pushes a full state snapshot over the `engine-state` event and answers
//! the `get_state` command with the same JSON.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use station_core::forwarder::Forwarder;
use station_core::save::{check_replay_autosave, ReplayAutoSaveStatus};
use station_core::set_machine::StatsProducer;

use crate::config::{self, Config};

/// Wording shared by the "Replay Auto Save is misconfigured" warning — used
/// both for the persistent status override and the one-time log line so the
/// two never drift apart.
fn replay_autosave_warning(raw: &str) -> String {
    format!(
        "Replay Auto Save is set to '{raw}' in Rivals 2 — set it to All or Local Only \
         (Options > Gameplay) or this app can't record any sets"
    )
}

const LOG_LINES: usize = 200;

pub struct Engine(pub Arc<EngineInner>);

pub struct EngineInner {
    app: AppHandle,
    pub config_dir: PathBuf,
    pub cfg: Mutex<Config>,
    rebuild: AtomicBool,
    status: Mutex<Value>, // {msg, error, t}
    log: Mutex<VecDeque<String>>,
    snapshot: Mutex<Value>,     // station producer snapshot {history, live}
    hub_snapshot: Mutex<Value>, // operator hub snapshot {sets, stations}
    hub_url: Mutex<Option<String>>,
    // The running hub (operator mode), for the report/swap/delete commands.
    hub: Mutex<Option<Arc<station_core::hub::Hub>>>,
    armed: AtomicBool, // stats save readable at least once

    // "Replay Auto Save" health check (see station_core::save). Tracked
    // separately from `status` above and re-applied as an override in
    // `state()` — see the comment on `check_replay_autosave` for why.
    replay_autosave_mtime: Mutex<Option<SystemTime>>,
    replay_autosave_bad: Mutex<Option<(String, i64)>>, // (raw enum value, first-seen t)
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
        // A confirmed-bad Replay Auto Save setting overrides whatever the
        // normal status flow last wrote — see `check_replay_autosave` for
        // why this can't just be another `set_status` call.
        let status = match &*self.replay_autosave_bad.lock().unwrap() {
            Some((raw, first_seen_t)) => json!({
                "msg": replay_autosave_warning(raw),
                "error": true,
                "t": first_seen_t,
            }),
            None => self.status.lock().unwrap().clone(),
        };
        json!({
            "config": cfg,
            "status": status,
            "snapshot": *self.snapshot.lock().unwrap(),
            "hubSnapshot": *self.hub_snapshot.lock().unwrap(),
            "hubUrl": *self.hub_url.lock().unwrap(),
            "log": self.log.lock().unwrap().iter().cloned().collect::<Vec<_>>(),
            "health": {
                "savePath": save.to_string_lossy(),
                "saveExists": save.is_file(),
                "saveArmed": self.armed.load(Ordering::Relaxed),
                "replaysPath": replays.to_string_lossy(),
                "replaysExists": replays.is_dir(),
                "outDir": out_dir.to_string_lossy(),
            },
        })
    }

    pub fn emit_state(&self) {
        let _ = self.app.emit("engine-state", self.state());
    }

    pub fn request_rebuild(&self) {
        self.rebuild.store(true, Ordering::SeqCst);
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

    fn out_dir(&self, cfg: &Config) -> PathBuf {
        if cfg.dir.is_empty() {
            self.config_dir.join("matchlogger-out")
        } else {
            PathBuf::from(&cfg.dir)
        }
    }
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
    hub_pieces: Option<crate::hub_glue::HubPieces>,
}

fn build(inner: &Arc<EngineInner>) -> Built {
    let cfg = inner.cfg.lock().unwrap().clone();

    // Operator side first: the local forwarder points at our own hub in
    // "both" mode (don't round-trip the LAN).
    let hub_pieces = if cfg.is_operator() {
        match crate::hub_glue::build_hub(inner, &cfg) {
            Ok(p) => {
                *inner.hub_url.lock().unwrap() = Some(p.url.clone());
                Some(p)
            }
            Err(e) => {
                inner.log_line(&format!("hub failed to start: {e}"));
                *inner.hub_url.lock().unwrap() = None;
                inner.set_hub(None);
                None
            }
        }
    } else {
        *inner.hub_url.lock().unwrap() = None;
        inner.set_hub(None);
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
            inner.log_line("local-only: no event/broker configured — nothing is sent");
        }
    }

    Built {
        producer,
        forwarder,
        hub_pieces,
    }
}

/// Create the engine and start its loop thread.
pub fn start(app: AppHandle, config_dir: PathBuf) -> Engine {
    let cfg = config::load(&config_dir);
    let inner = Arc::new(EngineInner {
        app,
        config_dir,
        cfg: Mutex::new(cfg),
        rebuild: AtomicBool::new(true),
        status: Mutex::new(
            json!({ "msg": "starting…", "error": false, "t": station_core::now_sec() }),
        ),
        log: Mutex::new(VecDeque::new()),
        snapshot: Mutex::new(json!({ "history": [], "live": null })),
        hub_snapshot: Mutex::new(json!({ "sets": [], "stations": {} })),
        hub_url: Mutex::new(None),
        hub: Mutex::new(None),
        armed: AtomicBool::new(false),
        replay_autosave_mtime: Mutex::new(None),
        replay_autosave_bad: Mutex::new(None),
    });

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
            }
            // mtime-gated, so this is a no-op stat() call on almost every
            // tick — only re-parses the settings file when it changes.
            loop_inner.check_replay_autosave();
            loop_inner.emit_state();
            let poll = loop_inner.cfg.lock().unwrap().poll.max(0.5);
            std::thread::sleep(std::time::Duration::from_secs_f64(poll));
        }
    });

    Engine(inner)
}
