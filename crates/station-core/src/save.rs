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

use uesave::{MapEntry, Property, PropertyKey, Save, StructValue};

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
}
