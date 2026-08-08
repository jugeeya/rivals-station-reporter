//! Typed mirror of the engine's `state()` JSON — the Rust counterpart of the
//! web app's types.ts. Loose where the wire is loose: hub set records stay
//! `serde_json::Value` (`HubRecord` there), everything the UI reads by name
//! is typed with defaults so a missing field can never panic a view.

use serde::Deserialize;
use serde_json::Value;

pub use crate::engine::config::Config;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Status {
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub error: bool,
    #[serde(default)]
    pub t: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SnapshotPlayer {
    #[serde(default)]
    pub tag: String,
    #[serde(default, rename = "char")]
    pub character: String,
    #[serde(default)]
    pub wins: i64,
    #[serde(default)]
    pub slot: i64,
    #[serde(default)]
    pub won: bool,
    /// Public tag database's resolved start.gg handle, when known.
    #[serde(default)]
    pub sgg: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SnapshotSet {
    #[serde(default, rename = "startEpoch")]
    pub start_epoch: i64,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub games: i64,
    #[serde(default)]
    pub players: Vec<SnapshotPlayer>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Snapshot {
    #[serde(default)]
    pub history: Vec<SnapshotSet>,
    #[serde(default)]
    pub live: Option<SnapshotSet>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HubSnapshot {
    /// Loose on purpose — same JSON the web console read (HubRecord).
    #[serde(default)]
    pub sets: Vec<Value>,
    #[serde(default)]
    pub stations: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Health {
    #[serde(default, rename = "savePath")]
    pub save_path: String,
    #[serde(default, rename = "saveExists")]
    pub save_exists: bool,
    #[serde(default, rename = "saveArmed")]
    pub save_armed: bool,
    #[serde(default, rename = "replaysPath")]
    pub replays_path: String,
    #[serde(default, rename = "replaysExists")]
    pub replays_exists: bool,
    #[serde(default, rename = "outDir")]
    pub out_dir: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EngineState {
    #[serde(default)]
    pub config: Config,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub snapshot: Snapshot,
    #[serde(default, rename = "hubSnapshot")]
    pub hub_snapshot: HubSnapshot,
    #[serde(default, rename = "hubUrl")]
    pub hub_url: Option<String>,
    #[serde(default)]
    pub log: Vec<String>,
    #[serde(default)]
    pub health: Health,
}

impl EngineState {
    pub fn from_value(v: &Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }
}

/// What `resolve_event` echoes back for a pasted start.gg URL.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EventSummary {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tournament: String,
    #[serde(default)]
    pub entrants: Option<i64>,
}

// ---- Current Sets (list_available_sets) --------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AvailableEntrant {
    #[serde(default)]
    pub id: Value,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AvailableSet {
    #[serde(default)]
    pub id: Value,
    #[serde(default)]
    pub state: Option<i64>,
    #[serde(default, rename = "fullRoundText")]
    pub full_round_text: String,
    #[serde(default)]
    pub station: Option<i64>,
    #[serde(default)]
    pub stream: Option<String>,
    /// Set from a bracket that hasn't been started on start.gg yet, which
    /// start.gg reports with a placeholder id ("preview_3396320_1_0"). It
    /// can't be assigned or started -- the attempt fails with a bare "An
    /// unknown error has occurred" -- so the row stays visible but its
    /// actions are disabled and labelled.
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    pub entrants: Vec<AvailableEntrant>,
    #[serde(default, rename = "startggStartedAt")]
    pub startgg_started_at: Option<i64>,
    #[serde(default, rename = "startggTotalGames")]
    pub startgg_total_games: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AvailableStation {
    #[serde(default)]
    pub number: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AvailableStream {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AvailableSets {
    #[serde(default)]
    pub sets: Vec<AvailableSet>,
    #[serde(default)]
    pub stations: Vec<AvailableStation>,
    #[serde(default)]
    pub streams: Vec<AvailableStream>,
}

/// A set id as the wire string the hub compares by — numbers print bare,
/// strings unquoted (the same normalization the hub's `py_str` applies).
pub fn id_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl AvailableSet {
    pub fn key(&self) -> String {
        id_str(&self.id)
    }

    pub fn players_label(&self) -> String {
        if self.entrants.is_empty() {
            return "?".into();
        }
        self.entrants
            .iter()
            .map(|e| if e.name.is_empty() { "?" } else { &e.name })
            .collect::<Vec<_>>()
            .join(" vs ")
    }
}
