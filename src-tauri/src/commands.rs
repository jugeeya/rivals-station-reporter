//! Command surface — request/response only; live data flows through the
//! `engine-state` event instead.

use serde_json::{json, Value};
use tauri::State;

use crate::config::{self, Config};
use crate::engine::Engine;

#[tauri::command]
pub fn get_state(engine: State<'_, Engine>) -> Value {
    engine.0.state()
}

#[tauri::command]
pub fn save_config(engine: State<'_, Engine>, cfg: Config) -> Result<Value, String> {
    config::save(&engine.0.config_dir, &cfg)?;
    *engine.0.cfg.lock().unwrap() = cfg;
    engine.0.set_status("settings saved", false);
    engine.0.request_rebuild();
    Ok(engine.0.state())
}

/// Echo back what a pasted start.gg URL points at (event + entrant count),
/// so a wrong paste is caught at setup time instead of at the first set.
#[tauri::command]
pub async fn resolve_event(url: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || station_core::startgg_web::event_summary(&url))
        .await
        .map_err(|e| e.to_string())?
}

/// Auto-detected save/replays locations plus whether they exist — the
/// onboarding pre-fills from this.
#[tauri::command]
pub fn default_paths() -> Value {
    let (save, replays) = config::default_save_paths();
    json!({
        "save": save.to_string_lossy(),
        "saveExists": save.is_file(),
        "replays": replays.to_string_lossy(),
        "replaysExists": replays.is_dir(),
    })
}

/// Sweep the local /24 for operator hubs, so a station doesn't have to be
/// told the operator's IP by hand.
///
/// Runs on a blocking thread: the sweep is ~1-2s of parallel HTTP and would
/// otherwise stall the UI. A machine that is itself the hub skips its own
/// address — offering to connect an operator to itself is just noise.
#[tauri::command]
pub async fn find_hubs(engine: State<'_, Engine>) -> Result<Value, String> {
    let (port, is_operator) = {
        let cfg = engine.0.cfg.lock().unwrap();
        (cfg.hub_port, cfg.is_operator())
    };
    let found = tauri::async_runtime::spawn_blocking(move || {
        let skip = is_operator
            .then(station_core::discovery::local_ipv4)
            .flatten();
        station_core::discovery::scan(port, skip)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(json!({ "hubs": found }))
}

// ---- operator actions (delegated to the hub) --------------------------------
// The winner-report is the ONE action that advances the bracket; it is always
// an explicit click in the UI, never automatic — same policy as every prior
// incarnation of this system.

#[tauri::command]
pub fn report_winner(
    engine: State<'_, Engine>,
    station: i64,
    set_id: String,
    winner_entrant_id: Value,
) -> Result<Value, String> {
    crate::hub_glue::do_report(&engine.0, station, &set_id, &winner_entrant_id)
}

#[tauri::command]
pub fn swap_players(
    engine: State<'_, Engine>,
    station: i64,
    set_id: String,
) -> Result<Value, String> {
    crate::hub_glue::do_swap(&engine.0, station, &set_id)
}

#[tauri::command]
pub fn delete_set(
    engine: State<'_, Engine>,
    station: i64,
    set_id: String,
) -> Result<Value, String> {
    crate::hub_glue::do_delete(&engine.0, station, &set_id)
}

// ---- autostart ---------------------------------------------------------------

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    if enabled { mgr.enable() } else { mgr.disable() }.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}
