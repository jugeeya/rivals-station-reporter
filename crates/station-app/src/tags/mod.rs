//! The Tag Installer's engine — ported from the Rivals 2 Tag Tool's
//! "Install tags to your setup" flow
//! (github.com/alex-mireles/rivals-2-tag-tool, PR #4). Headless; the screen
//! lives in `crate::ui::tag_installer`.
//!
//! The pieces: the published tag manifest + downloads (`site`), bracket
//! entrant lookup (`bracket`), and reading/writing the game's
//! `Rivals2_PlayerTagSaveSlot.sav` (`save`).

pub mod bracket;
pub mod save;
pub mod site;

use std::collections::HashSet;

/// Which published tags belong to a bracket's entrants, and which entrants
/// have nothing published. The join key is the start.gg user slug — exact,
/// like the VOD splitter's set-id join, never a name match.
pub struct BracketMatch {
    /// Manifest `file` names to pin/select.
    pub files: Vec<String>,
    /// Display names (gamer tag, falling back to entrant name) of entrants
    /// with no published tag.
    pub misses: Vec<String>,
}

pub fn match_bracket(manifest: &[site::SharedTag], entrants: &[bracket::Entrant]) -> BracketMatch {
    let slugs: HashSet<&str> = entrants.iter().map(|e| e.slug.as_str()).collect();
    let files: Vec<String> = manifest
        .iter()
        .filter(|t| !t.startgg_slug.is_empty() && slugs.contains(t.startgg_slug.as_str()))
        .map(|t| t.file.clone())
        .collect();

    let published: HashSet<&str> = manifest
        .iter()
        .map(|t| t.startgg_slug.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen = HashSet::new();
    let misses: Vec<String> = entrants
        .iter()
        .filter(|e| !published.contains(e.slug.as_str()))
        .filter(|e| seen.insert(e.slug.clone()))
        .map(|e| {
            if e.gamer_tag.is_empty() {
                e.entrant.clone()
            } else {
                e.gamer_tag.clone()
            }
        })
        .collect();

    BracketMatch { files, misses }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, file: &str, slug: &str) -> site::SharedTag {
        site::SharedTag {
            name: name.into(),
            author: String::new(),
            file: file.into(),
            startgg_slug: slug.into(),
            startgg_tag: name.into(),
        }
    }

    fn entrant(name: &str, slug: &str) -> bracket::Entrant {
        bracket::Entrant {
            entrant: name.into(),
            gamer_tag: name.into(),
            slug: slug.into(),
        }
    }

    #[test]
    fn matches_by_slug_and_reports_misses() {
        let manifest = vec![
            tag("kim", "kim.r2tag.zip", "user/aa"),
            tag("loom", "loom.r2tag.zip", "user/bb"),
            tag("unlinked", "x.r2tag.zip", ""),
        ];
        let entrants = vec![
            entrant("kim", "user/aa"),
            entrant("navi", "user/cc"),
            entrant("navi", "user/cc"), // duplicate participant rows collapse
        ];
        let m = match_bracket(&manifest, &entrants);
        assert_eq!(m.files, vec!["kim.r2tag.zip"]);
        assert_eq!(m.misses, vec!["navi"]);
    }

    #[test]
    fn empty_manifest_means_everyone_missing() {
        let m = match_bracket(&[], &[entrant("a", "user/aa")]);
        assert!(m.files.is_empty());
        assert_eq!(m.misses, vec!["a"]);
    }
}
