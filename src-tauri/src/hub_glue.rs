//! Operator-mode wiring: builds the LAN hub + HTTP server from config and
//! exposes the pieces the engine loop and the operator commands need.
//!
//! Mirrors the Python widget's `_build_hub` / `on_report` / `on_swap` /
//! `on_delete`: `players.json` (hand-written save-tag -> start.gg-tag
//! aliases) and `learned-tags.json` (corrections the operator made) live next
//! to the config; the hub merges learned over hand-written so a manual entry
//! can be corrected once. Reporting rebinds a not-reportable set first — the
//! TO may have pressed Start Match since it was recorded.

use std::sync::Arc;

use serde_json::{json, Value};
use station_core::hub::{Hub, HubServer};
use station_core::matching;

use crate::config::Config;
use crate::engine::EngineInner;

pub struct HubPieces {
    pub url: String,
    pub port: u16,
    server: HubServer,
}

impl HubPieces {
    pub fn stop(mut self) {
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

    let mut server = HubServer::new(hub.clone(), cfg.hub_port, "0.0.0.0");
    let url = server.start()?;
    inner.set_hub_snapshot(hub.snapshot());
    inner.set_hub(Some(hub));
    Ok(HubPieces {
        url,
        port: cfg.hub_port,
        server,
    })
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
