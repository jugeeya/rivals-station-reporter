//! "What changes if this tag is installed?" — extracts a compact digest of a
//! tag's gameplay settings and controller bindings from its parsed save tree
//! and diffs two digests per option, for the installer's Option | Old | New
//! table.
//!
//! Ported from the tag-sharing website's `tags/tagdiff.js` (which the tag
//! tool's `src/lib/tagdiff.ts` also mirrors). All three read the save through
//! uesave, so the JSON tree is identical and this is a straight port — keep
//! them in sync if the field list changes. One difference in use: the site
//! diffs a tag against the DEFAULT baseline; the installer diffs the incoming
//! tag against the same-name tag already in the save (falling back to the
//! bundled default baseline when the save doesn't have one yet).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// The default control settings, pre-extracted to digest form by the
/// website's `build_default_baseline.mjs` (same digest code). Bundled so the
/// "brand-new tag" case needs no network.
const DEFAULT_BASELINE: &str = include_str!("../../assets/control-defaults.json");

const ENUM_SETTINGS: [&str; 5] = [
    "RollSetting",
    "RightStickSetting",
    "AirParrySetting",
    "AirGrabSetting",
    "ItemTossSetting",
];
const NUM_SETTINGS: [&str; 1] = ["AirdodgeCardinalRoundingAngle"];
/// Boolean settings that DON'T use the usual `b` prefix ("Hold to Taunt"
/// serialises as a bare `HoldToTaunt` BoolProperty — see the website's
/// comment; confirmed against a real save).
const BOOL_SETTINGS: [&str; 1] = ["HoldToTaunt"];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Digest {
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    #[serde(default)]
    pub controllers: BTreeMap<String, ControllerDigest>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ControllerDigest {
    #[serde(default)]
    pub actions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub axes: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sensitivity {
    pub max: f64,
    /// Kept for shape-parity with the website's digest (only `max` is
    /// compared, there and here).
    #[serde(default)]
    #[allow(dead_code)]
    pub values: Vec<f64>,
}

/// One changed option: Option Name | Old | New.
#[derive(Debug, Clone)]
pub struct DiffItem {
    pub label: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone)]
pub struct DiffGroup {
    pub scope: String,
    pub items: Vec<DiffItem>,
}

#[derive(Debug, Clone, Default)]
pub struct TagDiff {
    pub count: usize,
    pub groups: Vec<DiffGroup>,
}

pub fn default_baseline() -> Digest {
    serde_json::from_str(DEFAULT_BASELINE).unwrap_or_default()
}

// ---- digest extraction (pure; mirrors the GVAS property tree) ---------------

/// uesave suffixes repeated property names with `_<index>`; compare bare.
fn strip(k: &str) -> &str {
    match k.rfind('_') {
        Some(i) if k[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < k.len() => &k[..i],
        _ => k,
    }
}

fn first<'a>(obj: &'a Value, prefix: &str) -> Option<&'a Value> {
    obj.as_object()?
        .iter()
        .find(|(k, _)| k.starts_with(prefix))
        .map(|(_, v)| v)
}

fn enum_short(v: &Value) -> Value {
    match v.as_str() {
        Some(s) if s.contains("::") => Value::String(s.rsplit("::").next().unwrap_or(s).into()),
        _ => v.clone(),
    }
}

fn round_to(v: f64, n: i32) -> f64 {
    let m = 10f64.powi(n);
    (v * m).round() / m
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Parsed save root (`{ save_game_type, properties }`) -> digest.
pub fn extract_digest(root: &Value) -> Digest {
    let mut digest = Digest::default();
    let Some(tag) = root
        .pointer("/properties/SavedPlayerTags_0")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    else {
        return digest;
    };
    let empty = Value::Null;
    let cs = first(tag, "ControlSettings").unwrap_or(&empty);

    // Global gameplay settings + toggles (scalar children of ControlSettings).
    if let Some(obj) = cs.as_object() {
        for (k, v) in obj {
            if v.is_object() || v.is_array() {
                continue;
            }
            let base = strip(k);
            if ENUM_SETTINGS.contains(&base) {
                digest.settings.insert(base.into(), enum_short(v));
            } else if NUM_SETTINGS.contains(&base) {
                let n = v.as_f64().unwrap_or(0.0);
                digest.settings.insert(base.into(), round_to(n, 4).into());
            } else if base.starts_with('b') || BOOL_SETTINGS.contains(&base) {
                digest.settings.insert(base.into(), Value::Bool(truthy(v)));
            }
        }
    }

    // Bindings: collect every action/axis mapping, bucket by input type.
    let mut actions: Vec<&Value> = Vec::new();
    let mut axes: Vec<&Value> = Vec::new();
    fn walk<'a>(o: &'a Value, actions: &mut Vec<&'a Value>, axes: &mut Vec<&'a Value>) {
        match o {
            Value::Object(map) => {
                if map.keys().any(|k| k.starts_with("ActionName")) {
                    actions.push(o);
                }
                if map.keys().any(|k| k.starts_with("AxisName")) {
                    axes.push(o);
                }
                for v in map.values() {
                    walk(v, actions, axes);
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v, actions, axes);
                }
            }
            _ => {}
        }
    }
    walk(tag, &mut actions, &mut axes);

    let bucket_of = |e: &Value| -> String {
        if first(e, "bKeyboardKey").map(truthy).unwrap_or(false) {
            "Keyboard".into()
        } else {
            first(e, "GamepadType")
                .map(enum_short)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "Unknown".into())
        }
    };
    let keyname_of = |e: &Value| -> String {
        first(e, "Key")
            .and_then(|k| first(k, "KeyName"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    for e in &actions {
        let bucket = bucket_of(e);
        let c = digest.controllers.entry(bucket).or_default();
        let name = first(e, "ActionName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mods: Vec<&str> = ["bShift", "bCtrl", "bAlt", "bCmd"]
            .iter()
            .filter(|m| first(e, m).map(truthy).unwrap_or(false))
            .map(|m| &m[1..])
            .collect();
        let label = if mods.is_empty() {
            keyname_of(e)
        } else {
            format!("{} +{}", keyname_of(e), mods.join(","))
        };
        let list = c.actions.entry(name).or_default();
        if !list.contains(&label) {
            list.push(label);
        }
    }
    for e in &axes {
        let bucket = bucket_of(e);
        let c = digest.controllers.entry(bucket).or_default();
        let name = first(e, "AxisName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let scale = first(e, "Scale").and_then(Value::as_f64).unwrap_or(0.0);
        let label = format!("{}(x{})", keyname_of(e), fmt_num(round_to(scale, 3)));
        let list = c.axes.entry(name).or_default();
        if !list.contains(&label) {
            list.push(label);
        }
    }

    // Per-controller sensitivity (max per-axis value flags a change).
    let cset = first(cs, "ControllerSettings").unwrap_or(&empty);
    let mut pairs: Vec<(String, &Value)> = Vec::new();
    if let Some(blocks) = first(cset, "ControllerSettings").and_then(Value::as_array) {
        for b in blocks.iter().filter(|b| b.is_object()) {
            let ty = b
                .get("key")
                .map(enum_short)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if let Some(val) = b.get("value") {
                pairs.push((ty, val));
            }
        }
    }
    if let Some(kb) = first(cset, "KeyboardSettings") {
        pairs.push(("Keyboard".into(), kb));
    }
    for (ty, val) in pairs {
        let mut sens: Vec<f64> = Vec::new();
        fn collect(o: &Value, sens: &mut Vec<f64>) {
            match o {
                Value::Object(map) => {
                    for (k, v) in map {
                        if strip(k) == "Sensitivity" {
                            if let Some(n) = v.as_f64() {
                                sens.push(round_to(n, 3));
                                continue;
                            }
                        }
                        collect(v, sens);
                    }
                }
                Value::Array(a) => {
                    for v in a {
                        collect(v, sens);
                    }
                }
                _ => {}
            }
        }
        if let Some(props) = first(val, "AxisProperties") {
            collect(props, &mut sens);
        }
        if !sens.is_empty() {
            let max = sens.iter().cloned().fold(f64::MIN, f64::max);
            digest.controllers.entry(ty).or_default().sensitivity =
                Some(Sensitivity { max, values: sens });
        }
    }

    // Stable ordering so digests compare cleanly.
    for c in digest.controllers.values_mut() {
        for list in c.actions.values_mut().chain(c.axes.values_mut()) {
            list.sort();
        }
    }
    digest
}

// ---- diff --------------------------------------------------------------------

const TYPE_ORDER: [&str; 8] = [
    "Keyboard",
    "Standard",
    "Xbox360",
    "XboxOne",
    "GameCube",
    "NintendoSwitchPro",
    "PS4",
    "PS5",
];

/// JSON numbers must compare numerically: the bundled baseline holds `20`
/// where a rounded digest holds `20.0`, and those are the same setting.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Per-option changes installing `new` over `old` would make. Follows the
/// website's rules exactly: only options the incoming tag carries are
/// compared, and a binding only reports when the old side bound the same
/// action differently.
pub fn diff_digests(new: &Digest, old: &Digest) -> TagDiff {
    let mut groups: Vec<DiffGroup> = Vec::new();

    let mut s_items: Vec<DiffItem> = Vec::new();
    for (k, v) in &new.settings {
        let old_v = old.settings.get(k).unwrap_or(&Value::Null);
        if !values_equal(v, old_v) {
            s_items.push(DiffItem {
                label: setting_label(k),
                old: enum_label(old_v),
                new: enum_label(v),
            });
        }
    }
    s_items.sort_by(|a, b| a.label.cmp(&b.label));
    if !s_items.is_empty() {
        groups.push(DiffGroup {
            scope: "Gameplay settings".into(),
            items: s_items,
        });
    }

    let mut types: Vec<&String> = new.controllers.keys().collect();
    types.sort_by_key(|t| {
        TYPE_ORDER
            .iter()
            .position(|o| o == &t.as_str())
            .map(|i| i + 1)
            .unwrap_or(99)
    });
    let empty = ControllerDigest::default();
    for t in types {
        let c = &new.controllers[t];
        let bc = old.controllers.get(t).unwrap_or(&empty);
        let mut items: Vec<DiffItem> = Vec::new();
        if let (Some(s), Some(bs)) = (&c.sensitivity, &bc.sensitivity) {
            if s.max != bs.max {
                items.push(DiffItem {
                    label: "Sensitivity".into(),
                    old: fmt_num(bs.max),
                    new: fmt_num(s.max),
                });
            }
        }
        for (name, keys) in c.actions.iter().chain(c.axes.iter()) {
            let old_keys = bc.actions.get(name).or_else(|| bc.axes.get(name));
            if let Some(bk) = old_keys {
                if bk != keys {
                    items.push(DiffItem {
                        label: camel(name),
                        old: key_list(bk),
                        new: key_list(keys),
                    });
                }
            }
        }
        if !items.is_empty() {
            groups.push(DiffGroup {
                scope: if t == "Keyboard" {
                    "Keyboard".into()
                } else {
                    format!("Controller · {}", camel(t))
                },
                items,
            });
        }
    }

    TagDiff {
        count: groups.iter().map(|g| g.items.len()).sum(),
        groups,
    }
}

// ---- friendly labels -----------------------------------------------------------

fn camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch == '_' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        out.push(ch);
    }
    out.trim().to_string()
}

fn setting_label(k: &str) -> String {
    match k {
        "RollSetting" => "Roll".into(),
        "RightStickSetting" => "Right stick".into(),
        "AirParrySetting" => "Air parry".into(),
        "AirGrabSetting" => "Air grab".into(),
        "ItemTossSetting" => "Item toss".into(),
        "AirdodgeCardinalRoundingAngle" => "Airdodge cardinal angle".into(),
        // Spelled out rather than left to camel(), which would render the
        // unprefixed name as "Hold To Taunt".
        "HoldToTaunt" => "Hold to taunt".into(),
        _ => camel(k.trim_start_matches('b').trim_end_matches("Enabled")),
    }
}

fn enum_label(v: &Value) -> String {
    match v {
        Value::Bool(true) => "On".into(),
        Value::Bool(false) => "Off".into(),
        Value::Null => "—".into(),
        Value::String(s) => match s.as_str() {
            "Nair" => "N-air".into(),
            "Nspecial" => "N-special".into(),
            "None" => "Off".into(),
            other => camel(other),
        },
        Value::Number(n) => n.as_f64().map(fmt_num).unwrap_or_else(|| n.to_string()),
        other => other.to_string(),
    }
}

/// JS-style number rendering: no trailing `.0` on whole values.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn key_label(raw: &str) -> String {
    // A trailing "(x0.8)" scale suffix survives the lookup.
    let (bare, scale) = match raw.rfind("(x") {
        Some(i) if raw.ends_with(')') => (raw[..i].trim(), Some(&raw[i..])),
        _ => (raw.trim(), None),
    };
    let lab: String = match bare {
        "SDL_GAMEPAD_BUTTON_SOUTH" => "South (A)".into(),
        "SDL_GAMEPAD_BUTTON_EAST" => "East (B)".into(),
        "SDL_GAMEPAD_BUTTON_WEST" => "West (X)".into(),
        "SDL_GAMEPAD_BUTTON_NORTH" => "North (Y)".into(),
        "SDL_GAMEPAD_BUTTON_LEFT_SHOULDER" => "L bumper".into(),
        "SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER" => "R bumper".into(),
        "SDL_GAMEPAD_BUTTON_BACK" => "Back".into(),
        "SDL_GAMEPAD_BUTTON_START" => "Start".into(),
        "SDL_GAMEPAD_BUTTON_DPAD_UP" => "D-pad ↑".into(),
        "SDL_GAMEPAD_BUTTON_DPAD_DOWN" => "D-pad ↓".into(),
        "SDL_GAMEPAD_BUTTON_DPAD_LEFT" => "D-pad ←".into(),
        "SDL_GAMEPAD_BUTTON_DPAD_RIGHT" => "D-pad →".into(),
        "SDL_GAMEPAD_AXIS_LEFTX" => "L-stick X".into(),
        "SDL_GAMEPAD_AXIS_LEFTY" => "L-stick Y".into(),
        "SDL_GAMEPAD_AXIS_RIGHTX" => "R-stick X".into(),
        "SDL_GAMEPAD_AXIS_RIGHTY" => "R-stick Y".into(),
        "SDL_GAMEPAD_AXIS_LEFT_TRIGGER" => "L trigger".into(),
        "SDL_GAMEPAD_AXIS_RIGHT_TRIGGER" => "R trigger".into(),
        "SpaceBar" => "Space".into(),
        b if b.starts_with("RivalsVirtualKey_") => camel(&b["RivalsVirtualKey_".len()..]),
        b => b.into(),
    };
    match scale {
        Some(sc) => format!("{lab} {sc}"),
        None => lab,
    }
}

fn key_list(keys: &[String]) -> String {
    keys.iter()
        .map(|k| key_label(k))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A minimal tag tree in uesave's serialized shape.
    fn root(air_parry: &str, tap_jump: bool, jump_key: &str) -> Value {
        json!({
            "save_game_type": "/Script/x",
            "properties": {
                "SavedPlayerTags_0": [{
                    "TagName_0": "KAZE",
                    "ControlSettings_0": {
                        "AirParrySetting_0": format!("EAirParrySetting::{air_parry}"),
                        "bTapJumpEnabled_0": tap_jump,
                        "AirdodgeCardinalRoundingAngle_0": 20.0,
                        "ControllerSettings_0": {
                            "ControllerSettings_0": [{
                                "key": "EGamepadType::GameCube",
                                "value": {"AxisProperties_0": {"Sensitivity_0": 1.25}}
                            }],
                        },
                    },
                    "Bindings_0": [{
                        "ActionName_0": "Jump",
                        "GamepadType_0": "EGamepadType::GameCube",
                        "Key_0": {"KeyName_0": jump_key},
                    }],
                }],
            },
        })
    }

    #[test]
    fn digest_extracts_settings_bindings_and_sensitivity() {
        let d = extract_digest(&root("Nspecial", true, "SDL_GAMEPAD_BUTTON_NORTH"));
        assert_eq!(d.settings["AirParrySetting"], json!("Nspecial"));
        assert_eq!(d.settings["bTapJumpEnabled"], json!(true));
        let gc = &d.controllers["GameCube"];
        assert_eq!(gc.actions["Jump"], vec!["SDL_GAMEPAD_BUTTON_NORTH"]);
        assert_eq!(gc.sensitivity.as_ref().unwrap().max, 1.25);
    }

    #[test]
    fn diff_reports_old_and_new_per_option() {
        let old = extract_digest(&root("Nspecial", false, "SDL_GAMEPAD_BUTTON_SOUTH"));
        let new = extract_digest(&root("Nair", true, "SDL_GAMEPAD_BUTTON_NORTH"));
        let diff = diff_digests(&new, &old);
        assert_eq!(diff.count, 3);

        let settings = &diff.groups[0];
        assert_eq!(settings.scope, "Gameplay settings");
        let parry = settings
            .items
            .iter()
            .find(|i| i.label == "Air parry")
            .unwrap();
        assert_eq!(
            (parry.old.as_str(), parry.new.as_str()),
            ("N-special", "N-air")
        );
        let tap = settings
            .items
            .iter()
            .find(|i| i.label == "Tap Jump")
            .unwrap();
        assert_eq!((tap.old.as_str(), tap.new.as_str()), ("Off", "On"));

        let gc = &diff.groups[1];
        assert_eq!(gc.scope, "Controller · Game Cube");
        assert_eq!(gc.items[0].label, "Jump");
        assert_eq!(gc.items[0].old, "South (A)");
        assert_eq!(gc.items[0].new, "North (Y)");
    }

    #[test]
    fn identical_digests_diff_to_nothing() {
        let a = extract_digest(&root("Nspecial", true, "SDL_GAMEPAD_BUTTON_NORTH"));
        let diff = diff_digests(&a, &a.clone());
        assert_eq!(diff.count, 0);
    }

    /// Manual check against a real published `.r2tag`: set RSR_R2TAG to a
    /// path and run with `--ignored`. Asserts the digest finds real settings
    /// and prints the diff-vs-default so the field list can be eyeballed
    /// against the website's rendering of the same tag.
    #[test]
    #[ignore]
    fn real_r2tag_digest_and_default_diff() {
        let path = std::env::var("RSR_R2TAG").expect("set RSR_R2TAG");
        let root = crate::tags::save::tag_root_json(std::path::Path::new(&path)).unwrap();
        let digest = extract_digest(&root);
        assert!(
            !digest.settings.is_empty() || !digest.controllers.is_empty(),
            "a real tag should yield a non-empty digest"
        );
        let d = diff_digests(&digest, &default_baseline());
        for g in &d.groups {
            println!("== {}", g.scope);
            for it in &g.items {
                println!("   {:<32} {:<24} -> {}", it.label, it.old, it.new);
            }
        }
        println!("{} change(s) vs default", d.count);
    }

    #[test]
    fn bundled_baseline_parses_and_covers_the_known_settings() {
        let d = default_baseline();
        assert!(!d.settings.is_empty(), "baseline settings present");
        assert_eq!(d.settings["RollSetting"], json!("Default"));
        assert!(d.controllers.contains_key("GameCube"));
    }

    #[test]
    fn integer_baseline_equals_rounded_float_digest() {
        // The baseline stores 20 (integer JSON); a digest stores 20.0. Those
        // must not report as a change.
        let new = extract_digest(&root("Nspecial", true, "SDL_GAMEPAD_BUTTON_NORTH"));
        let mut old = new.clone();
        old.settings
            .insert("AirdodgeCardinalRoundingAngle".into(), json!(20));
        assert_eq!(diff_digests(&new, &old).count, 0);
    }
}
