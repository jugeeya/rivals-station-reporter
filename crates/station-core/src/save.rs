//! Stats-save reading — port of `parse_stats()` from `rivals_stats.py`.
//!
//! The Python version hand-rolled a targeted GVAS reader (find the
//! `AllPlayerTagStats` marker and read properties from there). Here the whole
//! save is parsed with the `uesave` crate instead — the same parser the
//! tag-sharing website runs as wasm, which the Python reader was validated
//! against ("matches the site's uesave wasm exactly"). Deleting the hand-rolled
//! reader was the point of the rewrite.
//!
//! Output shape is unchanged: a flat
//! `{ "tag|char|mode|Category": number }` map for every real player-tag stat,
//! with synthetic aggregate tags (ALL TAGS / CUM) filtered out.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use uesave::{MapEntry, Property, PropertyKey, Root, Save, StructValue};

use crate::stats::{STAT_FIELDS, SYNTHETIC};

/// Best-effort string out of a map key / property (Name, Str, Enum, Byte
/// label — the save uses Name keys for characters and Enum keys for modes).
fn prop_str(p: &Property) -> Option<String> {
    match p {
        Property::Str(s) | Property::Name(s) | Property::Enum(s) => Some(s.clone()),
        Property::Byte(uesave::Byte::Label(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Best-effort number out of a property (the stat maps mix Int and Float).
fn prop_num(p: &Property) -> Option<f64> {
    match p {
        Property::Int(v) => Some(*v as f64),
        Property::Int8(v) => Some(*v as f64),
        Property::Int16(v) => Some(*v as f64),
        Property::Int64(v) => Some(*v as f64),
        Property::UInt8(v) => Some(*v as f64),
        Property::UInt16(v) => Some(*v as f64),
        Property::UInt32(v) => Some(*v as f64),
        Property::UInt64(v) => Some(*v as f64),
        Property::Float(v) => Some(v.0 as f64),
        Property::Double(v) => Some(v.0),
        _ => None,
    }
}

/// Parse `Rivals2_StatsSaveSlot.sav` bytes into the flat stat map.
///
/// Errors only on an unreadable container; an unexpected shape inside just
/// yields fewer keys (mirroring the Python reader's tolerance — a mid-write
/// file is retried on the next poll by the caller).
pub fn parse_stats(bytes: &[u8]) -> Result<HashMap<String, f64>, String> {
    let save = Save::read(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut flat = HashMap::new();

    let Some(Property::Array(uesave::ValueVec::Struct(tags))) = save
        .root
        .properties
        .0
        .get(&PropertyKey::from("AllPlayerTagStats"))
    else {
        return Ok(flat);
    };

    for sv in tags {
        let StructValue::Struct(props) = sv else {
            continue;
        };
        let name = match props.0.get(&PropertyKey::from("PlayerTagName")) {
            Some(p) => match prop_str(p) {
                Some(n) => n,
                None => continue,
            },
            None => continue,
        };
        if SYNTHETIC.contains(&name.as_str()) {
            continue;
        }
        for (cat, _) in STAT_FIELDS {
            let Some(Property::Map(entries)) = props.0.get(&PropertyKey::from(cat)) else {
                continue;
            };
            for MapEntry { key, value } in entries {
                let Some(ch) = prop_str(key) else { continue };
                // Each character maps to a struct holding a `Values` map of
                // EGameModeType -> number.
                let Property::Struct(StructValue::Struct(inner)) = value else {
                    continue;
                };
                let Some(Property::Map(values)) = inner.0.get(&PropertyKey::from("Values")) else {
                    continue;
                };
                for MapEntry { key: mk, value: mv } in values {
                    let Some(mode_key) = prop_str(mk) else {
                        continue;
                    };
                    let Some(val) = prop_num(mv) else { continue };
                    // "EGameModeType::LOCAL" -> "LOCAL"
                    let mode = mode_key.rsplit("::").next().unwrap_or(&mode_key);
                    flat.insert(format!("{name}|{ch}|{mode}|{cat}"), val);
                }
            }
        }
    }
    Ok(flat)
}

/// Sorted unique tag names present in a flat stat map.
pub fn tag_names(flat: &HashMap<String, f64>) -> Vec<String> {
    let mut names: Vec<String> = flat
        .keys()
        .filter_map(|k| k.split('|').next())
        .map(|s| s.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    names.sort();
    names
}

/// Result of checking the "Replay Auto Save" gameplay setting in the
/// *settings* save (`Rivals2_SettingsSaveSlot.sav` — a different file, in a
/// sibling `Settings\` folder, than the stats save the rest of this module
/// reads). Only a value of `All` or a `Local`-flavoured value produce local
/// replays; `Off` or an online-only value mean local replays are never
/// written, no matter how long this app waits, so its core feature — set
/// reconstruction, which now requires a replay (see set_machine.rs) —
/// silently never works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayAutoSaveStatus {
    /// Confirmed set to a value that writes local replays.
    Ok,
    /// Confirmed set to a value that does NOT write local replays. Carries
    /// the raw string after `::` verbatim — see `root_replay_autosave` for
    /// why only ONE of the four enum spellings is actually confirmed.
    BadValue(String),
    /// Settings file missing, unreadable, or the property wasn't found.
    /// NOT evidence of a problem (the player may simply not have launched
    /// the game yet, or be on a machine/OS with a different path) — callers
    /// must not warn on this, only on a confirmed `BadValue`.
    Unknown,
}

/// Classify a raw `ReplayAutoSave` enum string (already stripped of its
/// `ERivalsReplayAutoSaveSetting::` prefix).
///
/// Only the literal `All` has been confirmed against a real save file (a
/// settings save with Replay Auto Save set to All parses to exactly
/// `"ERivalsReplayAutoSaveSetting::All"`). The exact spelling of the other
/// three values — Off, Local Only, Online Only — is NOT confirmed; the
/// compiled game's asset pak doesn't expose enum literal names in a way that
/// could be checked ahead of time. So this matches by substring rather than
/// an exact list, deliberately erring toward flagging anything unrecognized:
///   * contains "off" -> bad (replays fully disabled)
///   * equals "all" -> good
///   * contains "local" -> good (covers LocalOnly/Local/etc regardless of
///     exact spelling)
///   * anything else (e.g. an online-only value, or a future/unknown
///     spelling) -> bad
fn classify_replay_autosave(raw: &str) -> ReplayAutoSaveStatus {
    let lower = raw.to_lowercase();
    if lower.contains("off") {
        ReplayAutoSaveStatus::BadValue(raw.to_string())
    } else if lower == "all" || lower.contains("local") {
        ReplayAutoSaveStatus::Ok
    } else {
        ReplayAutoSaveStatus::BadValue(raw.to_string())
    }
}

/// Pull `GameplaySettings.ReplayAutoSave` out of an already-parsed settings
/// save root and classify it. Split out from `check_replay_autosave` so
/// tests can build a `Root` in memory (via `uesave`'s public `Properties`
/// API) instead of round-tripping real GVAS bytes.
fn root_replay_autosave(root: &Root) -> ReplayAutoSaveStatus {
    let Some(Property::Struct(StructValue::Struct(gameplay))) = root
        .properties
        .0
        .get(&PropertyKey::from("GameplaySettings"))
    else {
        return ReplayAutoSaveStatus::Unknown;
    };
    let Some(prop) = gameplay.0.get(&PropertyKey::from("ReplayAutoSave")) else {
        return ReplayAutoSaveStatus::Unknown;
    };
    let Some(raw) = prop_str(prop) else {
        return ReplayAutoSaveStatus::Unknown;
    };
    // "ERivalsReplayAutoSaveSetting::All" -> "All"
    let value = raw.rsplit("::").next().unwrap_or(&raw);
    classify_replay_autosave(value)
}

/// Read `Rivals2_SettingsSaveSlot.sav` and check whether "Replay Auto Save"
/// is configured in a way that produces local replays. Any failure to
/// locate/read/parse the file, or find the property, yields `Unknown` —
/// never a false-positive warning.
pub fn check_replay_autosave(settings_path: &Path) -> ReplayAutoSaveStatus {
    let Ok(bytes) = std::fs::read(settings_path) else {
        return ReplayAutoSaveStatus::Unknown;
    };
    let Ok(save) = Save::read(&mut Cursor::new(bytes)) else {
        return ReplayAutoSaveStatus::Unknown;
    };
    root_replay_autosave(&save.root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reader parity against a real save, mirroring the optional tail of
    /// `test_rivals_stats.py`: set RSR_REAL_SAVE=/path/to/Rivals2_StatsSaveSlot.sav
    /// to run it (skipped silently otherwise — CI has no save file).
    #[test]
    fn parses_real_save_when_provided() {
        let Ok(path) = std::env::var("RSR_REAL_SAVE") else {
            return;
        };
        let bytes = std::fs::read(&path).expect("read RSR_REAL_SAVE");
        let flat = parse_stats(&bytes).expect("parse real save");
        assert!(!flat.is_empty(), "no stat keys parsed");
        assert!(flat.keys().all(|k| k.split('|').count() == 4));
        for t in tag_names(&flat) {
            assert!(
                !SYNTHETIC.contains(&t.as_str()),
                "synthetic tag {t} not filtered"
            );
        }
    }

    #[test]
    fn garbage_bytes_error_cleanly() {
        assert!(parse_stats(&[0u8; 32]).is_err());
    }

    /// Build a settings-save `Root` in memory, with `GameplaySettings` ->
    /// `ReplayAutoSave` set to the given raw enum string, the same shape a
    /// real `Rivals2_SettingsSaveSlot.sav` parses to (confirmed against a
    /// real file for the `All` case).
    fn settings_root(replay_auto_save_raw: &str) -> Root {
        let mut gameplay = uesave::Properties::default();
        gameplay.insert(
            "ReplayAutoSave",
            Property::Enum(replay_auto_save_raw.to_string()),
        );
        let mut props = uesave::Properties::default();
        props.insert(
            "GameplaySettings",
            Property::Struct(StructValue::Struct(gameplay)),
        );
        Root {
            save_game_type: "/Script/Rivals2.RivalsSettingsSaveGame".to_string(),
            properties: props,
        }
    }

    #[test]
    fn replay_autosave_all_is_ok() {
        // The one literal confirmed against a real save file.
        let root = settings_root("ERivalsReplayAutoSaveSetting::All");
        assert_eq!(root_replay_autosave(&root), ReplayAutoSaveStatus::Ok);
    }

    #[test]
    fn replay_autosave_off_is_bad_any_case() {
        for raw in [
            "ERivalsReplayAutoSaveSetting::Off",
            "ERivalsReplayAutoSaveSetting::OFF",
            "ERivalsReplayAutoSaveSetting::oFf",
        ] {
            let root = settings_root(raw);
            let want = raw.rsplit("::").next().unwrap().to_string();
            assert_eq!(
                root_replay_autosave(&root),
                ReplayAutoSaveStatus::BadValue(want),
                "raw={raw}"
            );
        }
    }

    #[test]
    fn replay_autosave_local_variants_are_ok() {
        // Exact spelling of the "local only" variant is unconfirmed — any
        // spelling containing "local" must pass.
        for raw in [
            "ERivalsReplayAutoSaveSetting::LocalOnly",
            "ERivalsReplayAutoSaveSetting::Local",
            "ERivalsReplayAutoSaveSetting::LocalOnlySave",
        ] {
            let root = settings_root(raw);
            assert_eq!(
                root_replay_autosave(&root),
                ReplayAutoSaveStatus::Ok,
                "raw={raw}"
            );
        }
    }

    #[test]
    fn replay_autosave_online_only_is_bad() {
        let raw = "ERivalsReplayAutoSaveSetting::OnlineOnly";
        let root = settings_root(raw);
        assert_eq!(
            root_replay_autosave(&root),
            ReplayAutoSaveStatus::BadValue("OnlineOnly".to_string())
        );
    }

    #[test]
    fn replay_autosave_missing_property_is_unknown() {
        // GameplaySettings present but no ReplayAutoSave key.
        let props = {
            let mut p = uesave::Properties::default();
            p.insert(
                "GameplaySettings",
                Property::Struct(StructValue::Struct(uesave::Properties::default())),
            );
            p
        };
        let root = Root {
            save_game_type: "/Script/Rivals2.RivalsSettingsSaveGame".to_string(),
            properties: props,
        };
        assert_eq!(root_replay_autosave(&root), ReplayAutoSaveStatus::Unknown);
    }

    #[test]
    fn replay_autosave_missing_file_is_unknown_not_a_warning() {
        let status = check_replay_autosave(Path::new(
            "C:/definitely/not/a/real/path/Rivals2_SettingsSaveSlot.sav",
        ));
        assert_eq!(status, ReplayAutoSaveStatus::Unknown);
        // Unknown must never be treated as a warning by callers.
        assert!(!matches!(status, ReplayAutoSaveStatus::BadValue(_)));
    }

    #[test]
    fn replay_autosave_garbage_bytes_is_unknown() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rsr_settings_test_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Rivals2_SettingsSaveSlot.sav");
        std::fs::write(&path, [0u8; 32]).unwrap();
        assert_eq!(check_replay_autosave(&path), ReplayAutoSaveStatus::Unknown);
        std::fs::remove_dir_all(&dir).ok();
    }
}
