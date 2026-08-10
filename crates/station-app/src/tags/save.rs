//! Reading and writing `Rivals2_PlayerTagSaveSlot.sav` — the game's saved
//! player tags — via `uesave`. Ported from the tag tool's `commands/tags.rs`
//! (install side only; exporting/sharing stayed in the tag tool).
//!
//! Everything here is synchronous and blocking (a save is megabytes and
//! parses in ~0.5s release / several seconds debug); callers run it off the
//! UI thread.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use uesave::{Property, PropertyKey, Save, StructValue, ValueVec};

pub const TAG_SAVE_FILE: &str = "Rivals2_PlayerTagSaveSlot.sav";

const DEFAULT_TAG_NAMES: [&str; 4] = ["Player1", "Player2", "Player3", "Player4"];

fn is_custom_tag(name: &str) -> bool {
    !DEFAULT_TAG_NAMES.contains(&name)
}

/// Where the tag save should be. The reporter already knows the STATS save's
/// location (auto-detected or configured) and the tag save is its sibling in
/// `SaveGames/` — deriving from it means the Deck/Proton prefix case works
/// for free. Falls back to the platform-default install location.
pub fn default_save_path(stats_save: &str) -> Option<PathBuf> {
    if !stats_save.is_empty() {
        let p = Path::new(stats_save).with_file_name(TAG_SAVE_FILE);
        if p.is_file() {
            return Some(p);
        }
    }
    let p = dirs::data_local_dir()?
        .join("Rivals2")
        .join("Saved")
        .join("SaveGames")
        .join(TAG_SAVE_FILE);
    p.is_file().then_some(p)
}

fn read_save(path: &Path) -> Result<Save, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    Save::read(&mut reader).map_err(|e| format!("{}: {e}", path.display()))
}

fn tag_name_of(sv: &StructValue) -> Option<&str> {
    if let StructValue::Struct(props) = sv {
        if let Some(Property::Str(name)) = props.0.get(&PropertyKey::from("TagName")) {
            return Some(name.as_str());
        }
    }
    None
}

/// Rename a tag in place, so two people sharing an in-game tag name can both
/// be installed (the caller supplies the disambiguated name, e.g. a start.gg
/// tag).
fn set_tag_name(sv: &mut StructValue, name: &str) {
    if let StructValue::Struct(props) = sv {
        if let Some(Property::Str(existing)) = props.0.get_mut(&PropertyKey::from("TagName")) {
            *existing = name.to_string();
        }
    }
}

/// The root `SaveVersion` (save-format version). Both the save and `.r2tag`
/// files (which are full saves) carry this; `None` if absent.
fn save_version(save: &Save) -> Option<i32> {
    match save
        .root
        .properties
        .0
        .get(&PropertyKey::from("SaveVersion"))
    {
        Some(Property::Int(v)) => Some(*v),
        _ => None,
    }
}

fn tag_structs(save: &Save) -> Result<&Vec<StructValue>, String> {
    match &save.root.properties["SavedPlayerTags"] {
        Property::Array(ValueVec::Struct(structs)) => Ok(structs),
        _ => Err("SavedPlayerTags is not a struct array".into()),
    }
}

/// The custom (non-Player1..4) tag names currently in the save.
pub fn tag_names(save_path: &Path) -> Result<Vec<String>, String> {
    let save = read_save(save_path)?;
    Ok(tag_structs(&save)?
        .iter()
        .filter_map(tag_name_of)
        .filter(|n| is_custom_tag(n))
        .map(str::to_string)
        .collect())
}

#[derive(Debug, Clone)]
pub struct TagPreview {
    pub path: PathBuf,
    pub tag_name: String,
    /// True only when the .r2tag's save-format version matches the save's.
    pub compatible: bool,
}

/// Read `.r2tag` files and return each one's tag name, flagging whether its
/// save-format version matches the destination save (a cross-version import
/// would fail to write or corrupt settings).
pub fn tag_previews(r2tag_paths: &[PathBuf], save_path: &Path) -> Result<Vec<TagPreview>, String> {
    let dest_version = save_version(&read_save(save_path)?);

    let mut previews = Vec::new();
    for path in r2tag_paths {
        let save = read_save(path)?;
        let version = save_version(&save);
        let name = tag_structs(&save)?
            .iter()
            .find_map(tag_name_of)
            .ok_or_else(|| format!("{}: no tag name found", path.display()))?;
        previews.push(TagPreview {
            path: path.clone(),
            tag_name: name.to_string(),
            compatible: version.is_some() && version == dest_version,
        });
    }
    Ok(previews)
}

#[derive(Debug, Clone)]
pub struct ImportInstruction {
    pub path: PathBuf,
    pub tag_name: String,
    pub overwrite: bool,
    /// Install the tag under a different in-save name. Used when two people
    /// share an in-game tag name — without it the second install would
    /// collide with (and overwrite, or be skipped against) the first.
    pub rename: Option<String>,
}

#[derive(Debug, Default)]
pub struct ImportResult {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
    /// Rejected because the .r2tag's save-format version differs from the
    /// destination save.
    pub incompatible: Vec<String>,
}

/// Import tags from `.r2tag` files into the save. Each instruction says
/// whether to overwrite if the name already exists.
pub fn import_tags(
    save_path: &Path,
    instructions: Vec<ImportInstruction>,
) -> Result<ImportResult, String> {
    let mut dest = read_save(save_path)?;
    let dest_version = save_version(&dest);

    let mut result = ImportResult::default();

    // Scope the mutable borrow of dest so dest.write() can proceed after.
    {
        let dest_structs = match &mut dest.root.properties["SavedPlayerTags"] {
            Property::Array(ValueVec::Struct(structs)) => structs,
            _ => return Err("SavedPlayerTags is not a struct array in the save".into()),
        };

        // Where the next installed tag goes. Slot 0 is the player's own tag
        // — the game treats it as theirs — so installs never displace it;
        // they land directly after it, in the order they were chosen.
        let mut insert_at = if dest_structs.is_empty() { 0 } else { 1 };

        for instruction in instructions {
            let install_name = instruction
                .rename
                .clone()
                .unwrap_or_else(|| instruction.tag_name.clone());

            let existing_pos = dest_structs
                .iter()
                .position(|sv| tag_name_of(sv) == Some(install_name.as_str()));

            if existing_pos.is_some() && !instruction.overwrite {
                result.skipped.push(instruction.tag_name);
                continue;
            }

            let r2tag_save = read_save(&instruction.path)?;

            // Reject cross-version imports.
            let source_version = save_version(&r2tag_save);
            if source_version.is_none() || source_version != dest_version {
                result.incompatible.push(instruction.tag_name);
                continue;
            }

            let mut tag_sv = tag_structs(&r2tag_save)?
                .iter()
                .find(|sv| tag_name_of(sv) == Some(instruction.tag_name.as_str()))
                .ok_or_else(|| {
                    format!(
                        "{}: tag '{}' not found",
                        instruction.path.display(),
                        instruction.tag_name
                    )
                })?
                .clone();

            if instruction.rename.is_some() {
                set_tag_name(&mut tag_sv, &install_name);
            }

            match existing_pos {
                // Overwrite in place — including slot 0, whose content may be
                // replaced even though nothing is allowed to displace it.
                Some(pos) => dest_structs[pos] = tag_sv,
                None => {
                    let at = insert_at.min(dest_structs.len());
                    dest_structs.insert(at, tag_sv);
                    insert_at = at + 1;
                }
            }

            result.imported.push(install_name);
        }
    }

    let out = File::create(save_path).map_err(|e| e.to_string())?;
    dest.write(&mut std::io::BufWriter::new(out))
        .map_err(|e| e.to_string())?;

    Ok(result)
}
