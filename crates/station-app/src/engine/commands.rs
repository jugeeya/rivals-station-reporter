//! The old Tauri command surface as plain functions. Same contracts, same
//! JSON shapes — the UI calls these (blocking ones from a background task
//! via `tokio::task::spawn_blocking`; see `ui`).

use std::sync::Arc;

use serde_json::{json, Value};

use super::config::{self, Config};
use super::core::EngineInner;
use super::hub_glue;

pub fn get_state(engine: &Arc<EngineInner>) -> Value {
    engine.state()
}

pub fn save_config(engine: &Arc<EngineInner>, cfg: Config) -> Result<Value, String> {
    config::save(&engine.config_dir, &cfg)?;
    *engine.cfg.lock().unwrap() = cfg;
    engine.set_status("settings saved", false);
    engine.request_rebuild();
    Ok(engine.state())
}

/// Echo back what a pasted start.gg URL points at (event + entrant count),
/// so a wrong paste is caught at setup time instead of at the first set.
/// Blocking (network).
pub fn resolve_event(url: &str) -> Result<Value, String> {
    station_core::startgg_web::event_summary(url)
}

/// Auto-detected save/replays locations plus whether they exist — the
/// onboarding pre-fills from this.
pub fn default_paths() -> Value {
    let (save, replays) = config::default_save_paths();
    json!({
        "save": save.to_string_lossy(),
        "saveExists": save.is_file(),
        "replays": replays.to_string_lossy(),
        "replaysExists": replays.is_dir(),
    })
}

/// Sweep the local /24 for operator hubs. Blocking (~1-2s of parallel HTTP).
pub fn find_hubs(engine: &Arc<EngineInner>) -> Value {
    let (port, is_operator) = {
        let cfg = engine.cfg.lock().unwrap();
        (cfg.hub_port, cfg.is_operator())
    };
    let skip = is_operator
        .then(station_core::discovery::local_ipv4)
        .flatten();
    let found = station_core::discovery::scan(port, skip);
    json!({ "hubs": found })
}

// ---- operator actions (delegated to the hub; all blocking) -------------------

pub fn report_winner(
    engine: &Arc<EngineInner>,
    station: i64,
    set_id: &str,
    winner_entrant_id: &Value,
) -> Result<Value, String> {
    hub_glue::do_report(engine, station, set_id, winner_entrant_id)
}

/// Report a bracket set that no station tracked — the Bracket screen's
/// Report button. Winner only; see `hub_glue::do_report_bracket_set`.
pub fn report_bracket_set(
    engine: &Arc<EngineInner>,
    set_id: &str,
    winner_entrant_id: &Value,
) -> Result<Value, String> {
    hub_glue::do_report_bracket_set(engine, set_id, winner_entrant_id)
}

pub fn swap_players(
    engine: &Arc<EngineInner>,
    station: i64,
    set_id: &str,
) -> Result<Value, String> {
    hub_glue::do_swap(engine, station, set_id)
}

pub fn delete_set(engine: &Arc<EngineInner>, station: i64, set_id: &str) -> Result<Value, String> {
    hub_glue::do_delete(engine, station, set_id)
}

pub fn list_available_sets(engine: &Arc<EngineInner>) -> Result<Value, String> {
    hub_glue::available_sets(engine)
}

pub fn start_match(
    engine: &Arc<EngineInner>,
    set_id: &str,
    station_number: Option<i64>,
    stream_name: Option<String>,
) -> Result<Value, String> {
    hub_glue::do_start_match(engine, set_id, station_number, stream_name)
}

pub fn reassign_destination(
    engine: &Arc<EngineInner>,
    set_id: &str,
    station_number: Option<i64>,
    stream_name: Option<String>,
) -> Result<Value, String> {
    hub_glue::do_reassign_destination(engine, set_id, station_number, stream_name)
}

// ---- autostart ----------------------------------------------------------------
// auto-launch is the same crate tauri-plugin-autostart wraps, so behavior
// (launch agent on macOS, registry Run key on Windows, .desktop on Linux)
// matches what the Tauri build did.

fn launcher() -> Result<auto_launch::AutoLaunch, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name("Rivals Station Reporter")
        .set_app_path(&exe.to_string_lossy())
        .build()
        .map_err(|e| e.to_string())
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let l = launcher()?;
    if enabled { l.enable() } else { l.disable() }.map_err(|e| e.to_string())
}

pub fn get_autostart() -> Result<bool, String> {
    launcher()?.is_enabled().map_err(|e| e.to_string())
}

// ---- learned tags ---------------------------------------------------------------
// The hub's remembered tag -> entrant corrections (learned-tags.json, flat
// object; see hub.rs's learn_aliases). Exposed so a wrong correction can be
// forgotten from Settings instead of hand-editing JSON on the operator PC.

pub fn learned_tags(engine: &Arc<EngineInner>) -> Vec<(String, String)> {
    let path = engine.config_dir.join("learned-tags.json");
    let map: serde_json::Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut out: Vec<(String, String)> = map
        .into_iter()
        .map(|(tag, v)| (tag, v.as_str().unwrap_or_default().to_string()))
        .collect();
    out.sort();
    out
}

pub fn forget_learned_tag(engine: &Arc<EngineInner>, tag: &str) -> Result<(), String> {
    let path = engine.config_dir.join("learned-tags.json");
    let mut map: serde_json::Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    map.remove(tag);
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    // The running hub keeps its own in-memory copy; a rebuild (instant now)
    // reloads from disk so the forgotten pairing stops applying immediately.
    engine.request_rebuild();
    Ok(())
}
