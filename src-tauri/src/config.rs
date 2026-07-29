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
    let base = rivals2_saved_dir();
    (
        base.join("SaveGames").join("Rivals2_StatsSaveSlot.sav"),
        base.join("Replays"),
    )
}

/// The *settings* save — a different file than the stats save above, in a
/// sibling `Settings\` folder under the same `Rivals2\Saved` base. Holds the
/// "Replay Auto Save" gameplay option this app depends on (see
/// `station_core::save::check_replay_autosave`).
pub fn default_settings_path() -> PathBuf {
    rivals2_saved_dir()
        .join("Settings")
        .join("Rivals2_SettingsSaveSlot.sav")
}

/// Steam's App ID for Rivals of Aether II. Linux has no native build, so the
/// game runs under Proton there; its "AppData" is a Windows-shaped tree
/// inside a Proton compatibility prefix keyed by this ID, not a real user
/// profile.
const RIVALS2_APPID: &str = "217000";

fn rivals2_saved_dir() -> PathBuf {
    // `if cfg!(...)` rather than `#[cfg(...)]` on this fn: both branches stay
    // compiled (and therefore both stay reachable from `cargo test`) on every
    // dev platform, so the Linux/Steam Deck probing logic below is exercised
    // by the test suite even on a Windows box, instead of being invisible to
    // the compiler until someone happens to test it on Linux.
    if cfg!(target_os = "linux") {
        linux_rivals2_saved_dir(&dirs_home())
    } else {
        // Windows: %LOCALAPPDATA%\Rivals2\Saved, same as the game itself
        // resolves it. Untouched by the Linux work above.
        //
        // macOS falls through to this same branch. Rivals 2 has no confirmed
        // native macOS install layout (no native build exists at time of
        // writing), so this was already just a guess before this change —
        // %LOCALAPPDATA% is unset there, so it silently degrades to
        // `$HOME/AppData/Local/Rivals2/Saved`, which doesn't exist on macOS
        // either. Left as-is per instructions rather than inventing an
        // unconfirmed macOS path; the existing "not found" fallback in the
        // UI already handles a nonexistent path gracefully.
        std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs_home().join("AppData").join("Local"))
            .join("Rivals2")
            .join("Saved")
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

// ---- Linux / Steam Deck (Proton) save-path detection -----------------------
//
// None of the helpers below are `cfg`-gated to Linux: each takes its inputs
// (home dir, a Steam root) as plain parameters, so they're ordinary testable
// Rust rather than OS-specific code that only compiles where it runs. Only
// the `rivals2_saved_dir` dispatcher above decides whether any of this is
// actually used at runtime.

/// Steam install roots to probe, in priority order. This order is also what
/// the "nothing found" fallback in `linux_rivals2_saved_dir` uses, so it
/// determines what candidate path the UI shows when detection fails outright.
///
/// - `~/.local/share/Steam` is the Steam Deck's own default and the standard
///   native-Linux Steam install location — checked first since it covers the
///   overwhelming majority of Linux/Deck users.
/// - `~/.steam/steam` and `~/.steam/root` are compatibility symlinks that
///   Steam itself creates (and some distro packages still rely on) pointing
///   at the real data directory; kept as a fallback for setups where the
///   first path isn't it.
/// - The Flatpak sandbox path is checked last: Flatpak Steam is uncommon
///   compared to the native install, and its data lives in a completely
///   separate tree, so it should only win when nothing more common matched.
fn linux_steam_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local").join("share").join("Steam"),
        home.join(".steam").join("steam"),
        home.join(".steam").join("root"),
        home.join(".var")
            .join("app")
            .join("com.valvesoftware.Steam")
            .join(".local")
            .join("share")
            .join("Steam"),
    ]
}

/// Every Steam library under one Steam root, in probe order: the root's own
/// `steamapps` first, then whatever additional library folders it declares
/// in `libraryfolders.vdf`.
///
/// This has to look past the default root at all because of how Steam Deck
/// storage works in practice: `compatdata` (the Proton prefix, and therefore
/// our save file) lives in the *same library* as the game itself, and on a
/// Deck it's extremely common to install games to the SD card rather than
/// the internal drive. A Deck with games on an SD card will have a perfectly
/// real `~/.local/share/Steam/steamapps/compatdata`, just without our app in
/// it — the one that matters is over in the card's library instead.
fn linux_library_paths(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    let vdf_path = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(contents) = std::fs::read_to_string(&vdf_path) {
        libs.extend(parse_library_folders_vdf(&contents));
    }
    libs
}

/// Pull every `"path"  "<value>"` entry out of a Steam `libraryfolders.vdf`.
///
/// This is deliberately not a general VDF/KeyValues parser (Valve's format
/// supports nesting, quoting edge cases and comments) — it just scans line by
/// line for the one key every version of this file has used to record a
/// library folder's location, which is all we need. Malformed or absent
/// input (missing file, half-written file, a future format change) simply
/// yields no additional libraries rather than an error: the Steam-root
/// libraries `linux_library_paths` already includes are always a usable
/// fallback on their own.
fn parse_library_folders_vdf(contents: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("\"path\"") {
            continue;
        }
        let mut after_key = line.splitn(2, "\"path\"");
        let Some(rest) = after_key.nth(1) else {
            continue;
        };
        // `rest` is everything after the key, e.g. `\t\t"/home/deck/…"`.
        // Splitting on `"` gives [before-first-quote, value, after] — the
        // value is index 1 regardless of how much whitespace preceded it.
        let mut quoted = rest.split('"');
        let Some(value) = quoted.nth(1) else {
            continue;
        };
        // VDF escapes a literal backslash as `\\`. Steam always writes native
        // (forward-slash) paths in a Linux-side file, so this is normally a
        // no-op, but unescape it anyway in case the file was ever touched by
        // Windows-side Steam (e.g. a synced/shared config).
        paths.push(PathBuf::from(value.replace("\\\\", "\\")));
    }
    paths
}

/// Every candidate `Rivals2/Saved` location, in probe order, without checking
/// which (if any) exist.
fn linux_saved_dir_candidates(home: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for root in linux_steam_roots(home) {
        for lib in linux_library_paths(&root) {
            candidates.push(
                lib.join("steamapps")
                    .join("compatdata")
                    .join(RIVALS2_APPID)
                    .join("pfx")
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("AppData")
                    .join("Local")
                    .join("Rivals2")
                    .join("Saved"),
            );
        }
    }
    candidates
}

/// The first candidate `Rivals2/Saved` dir that actually exists, or — if
/// detection finds nothing at all — the most likely candidate anyway (the
/// Steam Deck/standard-Linux default), so the UI has a sensible path to show
/// in its "not found" state instead of an empty string. Callers already
/// treat a nonexistent path this way (see `commands::default_paths` and
/// `engine::EngineInner::state`, which just report `saveExists`/`saveArmed`
/// off `Path::is_file`/`is_dir` rather than requiring the path to resolve).
fn linux_rivals2_saved_dir(home: &Path) -> PathBuf {
    let candidates = linux_saved_dir_candidates(home);
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| {
            candidates
                .into_iter()
                .next()
                .expect("linux_steam_roots always yields at least one candidate")
        })
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

#[cfg(test)]
mod linux_saved_dir_tests {
    use super::*;

    /// A fresh, empty scratch dir to stand in for `$HOME` — same
    /// pid-namespaced temp-dir pattern the rest of this workspace's tests use
    /// (see e.g. `station_core::save::tests`), so parallel test runs never
    /// collide with each other or with a real Steam install.
    fn temp_home(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rivals2_cfgtest_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn steam_roots_are_probed_in_priority_order() {
        let home = PathBuf::from("/home/deck");
        assert_eq!(
            linux_steam_roots(&home),
            vec![
                PathBuf::from("/home/deck/.local/share/Steam"),
                PathBuf::from("/home/deck/.steam/steam"),
                PathBuf::from("/home/deck/.steam/root"),
                PathBuf::from("/home/deck/.var/app/com.valvesoftware.Steam/.local/share/Steam"),
            ]
        );
    }

    #[test]
    fn library_paths_include_the_steam_root_itself_even_with_no_vdf() {
        let home = temp_home("nolib");
        let root = home.join("Steam"); // steamapps dir deliberately not created
        assert_eq!(linux_library_paths(&root), vec![root]);
    }

    #[test]
    fn library_paths_append_extra_libraries_from_libraryfolders_vdf() {
        let home = temp_home("withlib");
        let root = home.join("Steam");
        let steamapps = root.join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        let sdcard = home.join("run_media_sdcard").join("steamlibrary");
        std::fs::write(
            steamapps.join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"label\"\t\t\"\"\n\t}}\n}}\n",
                sdcard.to_string_lossy(),
            ),
        )
        .unwrap();

        assert_eq!(linux_library_paths(&root), vec![root, sdcard]);
    }

    #[test]
    fn library_paths_tolerate_an_absent_vdf_file() {
        let home = temp_home("novdf");
        let root = home.join("Steam");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        // No libraryfolders.vdf written at all.
        assert_eq!(linux_library_paths(&root), vec![root]);
    }

    #[test]
    fn parse_library_folders_vdf_extracts_path_values_among_other_keys() {
        let contents = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/data/steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"contentid\"\t\t\"123\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/sdcard/SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            parse_library_folders_vdf(contents),
            vec![
                PathBuf::from("/data/steam"),
                PathBuf::from("/mnt/sdcard/SteamLibrary"),
            ]
        );
    }

    #[test]
    fn parse_library_folders_vdf_tolerates_malformed_or_unrelated_content() {
        assert!(parse_library_folders_vdf("").is_empty());
        assert!(parse_library_folders_vdf("not vdf at all\n{{{{").is_empty());
        // A "path" line with no closing quote must not panic.
        assert!(parse_library_folders_vdf("\"path\" no_closing_quote").is_empty());
    }

    #[test]
    fn candidates_probe_the_default_root_before_any_libraryfolders_entries() {
        let home = PathBuf::from("/home/deck");
        let candidates = linux_saved_dir_candidates(&home);
        assert_eq!(
            candidates[0],
            PathBuf::from(
                "/home/deck/.local/share/Steam/steamapps/compatdata/217000/pfx/drive_c/users/steamuser/AppData/Local/Rivals2/Saved"
            )
        );
    }

    #[test]
    fn finds_the_saved_dir_inside_an_sd_card_library_declared_in_the_vdf() {
        let home = temp_home("sdhit");
        let root = home.join(".local").join("share").join("Steam");
        let steamapps = root.join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        let sdcard = home.join("run_media_mmcblk0p1").join("steamlibrary");
        std::fs::write(
            steamapps.join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                sdcard.to_string_lossy(),
            ),
        )
        .unwrap();

        // The default root's own compatdata does NOT contain our app — only
        // the SD card library does, mirroring a real Deck-with-SD-card setup.
        let saved_dir = sdcard
            .join("steamapps")
            .join("compatdata")
            .join("217000")
            .join("pfx")
            .join("drive_c")
            .join("users")
            .join("steamuser")
            .join("AppData")
            .join("Local")
            .join("Rivals2")
            .join("Saved");
        std::fs::create_dir_all(&saved_dir).unwrap();

        assert_eq!(linux_rivals2_saved_dir(&home), saved_dir);
    }

    #[test]
    fn falls_back_to_the_default_candidate_when_nothing_exists() {
        let home = temp_home("nothingfound");
        // No Steam directories at all under this home — a machine that
        // has never installed Steam, or where nothing matched.
        let expected = linux_saved_dir_candidates(&home)[0].clone();
        assert_eq!(linux_rivals2_saved_dir(&home), expected);
    }
}
