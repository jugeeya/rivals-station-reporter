//! App configuration — the same keys as the Python reporter's `config.json`,
//! so a config carried over from an old install loads as-is.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_BROKER: &str = "https://r2tag-broker.jdsambasivam.workers.dev";
pub const DEFAULT_HUB_PORT: u16 = 8787;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// station | operator | both — same trichotomy as the Python widget.
    pub mode: String,
    pub station: i64,
    /// Hub / broker URL the forwarder posts to (a LAN hub or the Cloudflare
    /// broker — same API either way).
    pub broker: String,
    /// start.gg event slug (or empty: local scoreboard only, nothing sent).
    pub slug: String,
    /// Shared key — same value as the hub/broker's OPERATOR_KEY. Required to
    /// send; the live-score push is a real (non-advancing) bracket write.
    pub key: String,
    /// start.gg API token — operator only; never leaves the operator machine.
    pub startgg_token: String,
    /// Stats save path; empty = auto-detect (%LOCALAPPDATA%).
    pub save: String,
    /// Replays folder; empty = auto-detect.
    pub replays: String,
    /// Output folder the set machine writes (current/live/sets files).
    /// Empty = `<app data dir>/matchlogger-out`.
    pub dir: String,
    /// Finalize an open set after this many idle seconds. Measured across
    /// ten consecutive real online games: every set closed at exactly
    /// 180-182s (the old default) while the next game of the SAME set arrived
    /// only 2-118s later — i.e. games within a set are ~182-300s apart, so a
    /// 180s idle timer always fires first and splits real sets. 420s clears
    /// that observed gap with headroom.
    pub idle: f64,
    /// Seconds between engine passes.
    pub poll: f64,
    pub hub_port: u16,
    pub dry_run: bool,
    /// True once the user has completed first-run setup.
    pub configured: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: "station".into(),
            station: 1,
            broker: DEFAULT_BROKER.into(),
            slug: String::new(),
            key: String::new(),
            startgg_token: String::new(),
            save: String::new(),
            replays: String::new(),
            dir: String::new(),
            idle: 420.0,
            poll: 2.0,
            hub_port: DEFAULT_HUB_PORT,
            dry_run: false,
            configured: false,
        }
    }
}

impl Config {
    pub fn is_station(&self) -> bool {
        self.mode == "station" || self.mode == "both"
    }

    pub fn is_operator(&self) -> bool {
        self.mode == "operator" || self.mode == "both"
    }
}

/// Standard %LOCALAPPDATA% locations for the stats save and replays folder.
pub fn default_save_paths() -> (PathBuf, PathBuf) {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join("AppData").join("Local"))
        .join("Rivals2")
        .join("Saved");
    (
        base.join("SaveGames").join("Rivals2_StatsSaveSlot.sav"),
        base.join("Replays"),
    )
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn load(config_dir: &Path) -> Config {
    let path = config_dir.join("config.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(config_dir: &Path, cfg: &Config) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let path = config_dir.join("config.json");
    let tmp = config_dir.join("config.json.tmp");
    let body = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}
