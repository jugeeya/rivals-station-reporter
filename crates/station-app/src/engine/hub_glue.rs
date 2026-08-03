//! Operator-mode wiring: builds the LAN hub + HTTP server from config and
//! exposes the pieces the engine loop and the operator commands need.
//!
//! Mirrors the Python widget's `_build_hub` / `on_report` / `on_swap` /
//! `on_delete`: `players.json` (hand-written save-tag -> start.gg-tag
//! aliases) and `learned-tags.json` (corrections the operator made) live next
//! to the config; the hub merges learned over hand-written so a manual entry
//! can be corrected once. The engine's `TagDb` (`station_core::tagdb`) fills
//! in a third, lowest-precedence layer below both. Reporting rebinds a
//! not-reportable set first — the TO may have pressed Start Match since it
//! was recorded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use station_core::hub::{Hub, HubServer};
use station_core::matching;

use crate::engine::config::Config;
use crate::engine::core::EngineInner;

/// How often the background sweep (`spawn_reported_elsewhere_sweep`) wakes up
/// to check for a set finalized directly on start.gg's own page. start.gg
/// allows ~80 requests/60s per token (see `startgg.rs`'s `MIN_INTERVAL_S`
/// comment), and the awaiting-report list is normally small (a handful of
/// sets at most), so one sweep costs at most a handful of `set_state` calls --
/// negligible against that budget even stacked on top of this hub's other
/// per-station traffic (live pushes, heartbeats). 90 seconds is picked as a
/// middle point in the 60-120s range: frequent enough that an operator does
/// not sit looking at a stale "awaiting report" row for long after a TO
/// finalizes it directly on start.gg, without adding meaningful load.
const SWEEP_INTERVAL_S: u64 = 90;

pub struct HubPieces {
    pub url: String,
    pub port: u16,
    server: HubServer,
    sweep_stop: Arc<AtomicBool>,
}

impl HubPieces {
    pub fn stop(mut self) {
        // Every rebuild (any config save) spawns a fresh sweep thread; without
        // this signal the old one never learns it should exit and just keeps
        // ticking forever against an orphaned `Arc<Hub>` and a stale slug.
        self.sweep_stop.store(true, Ordering::SeqCst);
        self.server.stop();
    }
}

pub fn build_hub(inner: &Arc<EngineInner>, cfg: &Config) -> Result<HubPieces, String> {
    let here = &inner.config_dir;

    // Optional hand-written aliases, exactly like the Python widget.
    let aliases: Value = std::fs::read_to_string(here.join("players.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let tag_map = matching::build_tag_map(if aliases.is_null() {
        None
    } else {
        Some(&aliases)
    });

    let log_inner = inner.clone();
    let snap_inner = inner.clone();
    let hub = Arc::new(Hub::new(
        Some(&cfg.key),
        (!cfg.startgg_token.is_empty()).then(|| cfg.startgg_token.clone()),
        Some(tag_map),
        // Lowest-precedence alias source — see `Hub::new`'s doc comment for
        // the full precedence order. The engine owns the one `TagDb` so a
        // pure `station` install (no hub at all) can use the same map for
        // the snapshot's `sgg` field — see `EngineInner::tagdb_map`.
        Some(inner.tagdb_map()),
        Some(here.join("hub-state.json").to_string_lossy().into_owned()),
        Some(Box::new(move |m| log_inner.log_line(m))),
        Some(Box::new(move |snap| {
            snap_inner.set_hub_snapshot(snap.clone());
            snap_inner.emit_state();
        })),
        Some(
            here.join("learned-tags.json")
                .to_string_lossy()
                .into_owned(),
        ),
    ));

    // So `/matchlogger/health` — and therefore a station's LAN discovery scan
    // — reports which event this hub is running, instead of a station
    // auto-connecting to an anonymous address.
    hub.set_event_slug(&cfg.slug);

    let mut server = HubServer::new(hub.clone(), cfg.hub_port, "0.0.0.0");
    // Bind before spawning the sweep: if the port is taken, `?` returns here,
    // and a sweep spawned earlier would have no owner left to stop it — a
    // leaked thread polling start.gg every 90s against an orphaned hub, plus
    // one more per failed rebuild.
    let url = server.start()?;
    let sweep_stop = spawn_reported_elsewhere_sweep(&hub);
    inner.set_hub_snapshot(hub.snapshot());
    inner.set_hub(Some(hub));
    Ok(HubPieces {
        url,
        port: cfg.hub_port,
        server,
        sweep_stop,
    })
}

/// Background safety net for a set someone finalized directly on start.gg's
/// own page: without this, a set sitting in "awaiting report" stays looking
/// actionable indefinitely, since `do_report`'s own check only runs at the
/// moment someone clicks Report. Modeled on `station_core::tagdb::TagDb`'s
/// `spawn_refresh` (`Arc<Self>`, `thread::spawn` loop, `thread::sleep`):
/// always spawns, but each tick only does real work when there is something
/// to check, exactly like `TagDb`'s own refresher only fetches when a refresh
/// is actually due. `Hub` itself stays a plain struct with a callable
/// `sweep_reported_elsewhere` method; this Tauri-side glue owns the thread's
/// lifetime, not `Hub::new` -- the same split as `TagDb::load` (no thread)
/// versus `engine.rs` calling `spawn_refresh` separately.
fn spawn_reported_elsewhere_sweep(hub: &Arc<Hub>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let hub = hub.clone();
    thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            // Nothing to check without both a token (start.gg calls need one)
            // and a configured event slug (there's no bucket to sweep without
            // one) -- mirrors TagDb's own "only do real work when due" gate.
            if hub.startgg.enabled() {
                if let Some(slug) = hub.event_slug() {
                    hub.sweep_reported_elsewhere(&slug);
                }
            }
            // Sleep in 1s increments rather than one long sleep so a rebuild
            // stops this thread promptly instead of leaving it (and the
            // `Arc<Hub>` it holds) alive for up to SWEEP_INTERVAL_S more.
            for _ in 0..SWEEP_INTERVAL_S {
                if stop_thread.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
    stop
}

fn hub_and_slug(inner: &Arc<EngineInner>) -> Result<(Arc<Hub>, String), String> {
    let hub = inner
        .hub()
        .ok_or_else(|| "operator mode is not running".to_string())?;
    let slug = inner.cfg.lock().unwrap().slug.clone();
    Ok((hub, slug))
}

fn err_text(e: (Value, u16)) -> String {
    e.0["error"].as_str().unwrap_or("hub error").to_string()
}

/// Finalize on start.gg — the one action that advances the bracket, so it is
/// only ever called from an explicit click in the UI.
pub fn do_report(
    inner: &Arc<EngineInner>,
    station: i64,
    set_id: &str,
    winner: &Value,
) -> Result<Value, String> {
    let (hub, slug) = hub_and_slug(inner)?;
    let sid = json!(set_id);
    // May have become reportable since (TO pressed Start Match) — rebind first.
    if let Some(rec) = hub.get_set(&slug, station, &sid) {
        if rec["reportable"] == false {
            let _ = hub.rebind(&slug, station, &sid);
        }
    }
    hub.do_report(&slug, station, &sid, winner)
        .map_err(err_text)
}

/// The station guessed the two players backwards — flip the mapping and
/// re-push the corrected live score (the hub remembers the correction).
pub fn do_swap(inner: &Arc<EngineInner>, station: i64, set_id: &str) -> Result<Value, String> {
    let (hub, slug) = hub_and_slug(inner)?;
    hub.do_swap(&slug, station, &json!(set_id))
        .map_err(err_text)
}

/// Remove a set from the console. start.gg is never touched.
pub fn do_delete(inner: &Arc<EngineInner>, station: i64, set_id: &str) -> Result<Value, String> {
    let (hub, slug) = hub_and_slug(inner)?;
    hub.do_delete(&slug, station, &json!(set_id))
        .map_err(err_text)
}

/// Sets available to start right now (both entrants determined, not yet
/// begun) plus the event's stations, for the Start Match panel. Read-only.
pub fn available_sets(inner: &Arc<EngineInner>) -> Result<Value, String> {
    let (hub, slug) = hub_and_slug(inner)?;
    hub.available_sets(&slug).map_err(err_text)
}

/// Start a match on start.gg -- explicit operator action, same standing as
/// `do_report`. Optionally (re)assigns a station, a stream, or both first,
/// for each requested one that differs from the set's current assignment
/// (see `Hub::do_start_match` for the resolve-then-assign-then-start
/// sequence).
pub fn do_start_match(
    inner: &Arc<EngineInner>,
    set_id: &str,
    station_number: Option<i64>,
    stream_name: Option<String>,
) -> Result<Value, String> {
    let (hub, slug) = hub_and_slug(inner)?;
    hub.do_start_match(&slug, &json!(set_id), station_number, stream_name)
        .map_err(err_text)
}

/// Change a set's station and/or stream on start.gg without starting it --
/// explicit operator action, same standing as `do_start_match`. See
/// `Hub::do_reassign_destination`.
pub fn do_reassign_destination(
    inner: &Arc<EngineInner>,
    set_id: &str,
    station_number: Option<i64>,
    stream_name: Option<String>,
) -> Result<Value, String> {
    let (hub, slug) = hub_and_slug(inner)?;
    hub.do_reassign_destination(&slug, &json!(set_id), station_number, stream_name)
        .map_err(err_text)
}
