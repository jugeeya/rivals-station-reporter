//! The splitter screen's own remembered bits — last slug, tournament display
//! name, a custom hub-state file if one was picked. Kept in its own
//! `vod-splitter.json` beside the app config rather than inside it: the
//! engine's config is engine state (a save rebuilds the hub), while these are
//! pure UI conveniences that should never trigger a rebuild.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub last_slug: String,
    #[serde(default)]
    pub tournament_name: String,
    /// A sets folder picked by hand (splitting a VOD recorded on some other
    /// machine). Empty = use this app's own `<out dir>/sets`.
    #[serde(default)]
    pub sets_dir: String,
}

fn path_in(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("vod-splitter.json")
}

impl Prefs {
    /// Best-effort load; a missing or corrupt file just yields defaults, since
    /// losing remembered fields should never stop the screen from opening.
    pub fn load(config_dir: &Path) -> Self {
        std::fs::read_to_string(path_in(config_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) {
        let Ok(json) = serde_json::to_string_pretty(self) else {
            return;
        };
        let _ = std::fs::create_dir_all(config_dir);
        let _ = std::fs::write(path_in(config_dir), json);
    }
}
