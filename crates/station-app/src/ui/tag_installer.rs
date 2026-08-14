//! The Tag Installer screen — the Rivals 2 Tag Tool's "Install tags to your
//! setup" flow (github.com/alex-mireles/rivals-2-tag-tool, PR #4), folded in.
//!
//! Before a bracket starts, a setup PC can pull every entrant's saved tag
//! (name, colors, controls) from the published tag database and install them
//! into the game's own save:
//!
//!   1. The tag save (`Rivals2_PlayerTagSaveSlot.sav`) is found automatically
//!      — it sits next to the stats save this app already watches.
//!   2. Paste the bracket URL and Find: entrants are matched to published
//!      tags by their start.gg account (exact, never by name), matches are
//!      selected, and whoever has nothing published is listed.
//!   3. Install: download, check save-format compatibility, and write into
//!      the save — overwriting same-named tags by default, and renaming to
//!      the start.gg tag when two people share an in-game name so both land.
//!
//! Local `.r2tag` files can be installed the same way, no database involved.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Center, Element, Fill, Length, Task};

use serde::Deserialize;

use super::{blocking, App, Message};
use crate::tags::{bracket, diff, match_bracket, save, site};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Good,
    Warn,
    Bad,
}

pub struct State {
    /// The game's tag save. Auto-detected on open; `Choose…` overrides.
    save_path: Option<PathBuf>,
    /// Custom tag names currently in the save (`None` until read).
    save_tags: Option<Vec<String>>,

    manifest: Vec<site::SharedTag>,
    manifest_loaded: bool,
    fetching_manifest: bool,

    search: String,
    /// Selected manifest `file` names.
    selected: HashSet<String>,
    /// Rows the last bracket Find matched — floated to the top of the list.
    pinned: HashSet<String>,

    bracket_url: String,
    bracket_busy: bool,
    /// Entrants with no published tag, from the last Find.
    misses: Vec<String>,
    show_misses: bool,

    /// Installing overwrites same-named tags already in the save by default;
    /// the checkbox next to Install opts out.
    overwrite: bool,
    installing: bool,

    /// Which manifest row's Option | Old | New table is expanded, if any.
    open_diff: Option<String>,
    /// Computed diffs, cached per manifest file for the session.
    diffs: HashMap<String, DiffState>,

    pub status: String,
    tone: Tone,
}

#[derive(Debug, Clone)]
pub enum Msg {
    ChooseSave,
    SavePicked(Option<PathBuf>),
    SaveTagsRead(Result<Vec<String>, String>),

    RefreshManifest,
    ManifestFetched(Result<Vec<ManifestTag>, String>),

    SearchChanged(String),
    ToggleTag(String, bool),
    ClearSelection,

    BracketUrlChanged(String),
    FindBracket,
    BracketFound(Result<BracketOutcome, String>),
    ToggleMisses,

    OverwriteToggled(bool),
    ToggleDiff(String),
    DiffReady(String, Result<TagChanges, String>),
    Install,
    PickR2tagFiles,
    R2tagsPicked(Option<Vec<PathBuf>>),
    Installed(Result<String, String>),
}

/// `site::SharedTag` twin that satisfies Msg's `Clone + Debug` without
/// leaking reqwest types into the message enum.
#[derive(Debug, Clone)]
pub struct ManifestTag(pub site::SharedTag);

#[derive(Debug, Clone)]
pub struct BracketOutcome {
    pub event: String,
    pub files: Vec<String>,
    pub misses: Vec<String>,
    pub entrant_count: usize,
}

/// A computed Option | Old | New table for one manifest row.
#[derive(Debug, Clone)]
pub struct TagChanges {
    /// The tag's in-game name, for the panel heading.
    pub tag_name: String,
    /// True when "Old" is the same-name tag already in this save; false when
    /// the save has none yet and the bundled default settings stand in.
    pub vs_save: bool,
    pub diff: diff::TagDiff,
}

enum DiffState {
    Loading,
    Ready(TagChanges),
    Failed(String),
}

impl Default for State {
    fn default() -> Self {
        Self {
            save_path: None,
            save_tags: None,
            manifest: Vec::new(),
            manifest_loaded: false,
            fetching_manifest: false,
            search: String::new(),
            selected: HashSet::new(),
            pinned: HashSet::new(),
            bracket_url: String::new(),
            bracket_busy: false,
            misses: Vec::new(),
            show_misses: false,
            overwrite: true,
            installing: false,
            open_diff: None,
            diffs: HashMap::new(),
            status: "Paste a bracket URL to select everyone's tags at once.".into(),
            tone: Tone::Plain,
        }
    }
}

impl State {
    fn say(&mut self, msg: impl Into<String>, tone: Tone) {
        self.status = msg.into();
        self.tone = tone;
    }

    /// Manifest rows matching the search, bracket matches first (stable).
    fn visible(&self) -> Vec<&site::SharedTag> {
        let q = self.search.trim().to_lowercase();
        let mut rows: Vec<&site::SharedTag> = self
            .manifest
            .iter()
            .filter(|t| {
                q.is_empty()
                    || t.name.to_lowercase().contains(&q)
                    || t.startgg_tag.to_lowercase().contains(&q)
                    || t.author.to_lowercase().contains(&q)
            })
            .collect();
        if !self.pinned.is_empty() {
            rows.sort_by_key(|t| !self.pinned.contains(&t.file));
        }
        rows
    }

    fn read_save_tags(&self) -> Task<Msg> {
        let Some(path) = self.save_path.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { blocking(move || save::tag_names(&path)).await },
            Msg::SaveTagsRead,
        )
    }

    fn fetch_manifest(&mut self) -> Task<Msg> {
        self.fetching_manifest = true;
        Task::perform(site::fetch_shared_tags(), |r| {
            Msg::ManifestFetched(r.map(|tags| tags.into_iter().map(ManifestTag).collect()))
        })
    }

    fn step(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::ChooseSave => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose Rivals2_PlayerTagSaveSlot.sav")
                        .add_filter("Rivals 2 save", &["sav"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Msg::SavePicked,
            ),
            Msg::SavePicked(None) => Task::none(),
            Msg::SavePicked(Some(path)) => {
                self.save_path = Some(path);
                self.save_tags = None;
                self.read_save_tags()
            }
            Msg::SaveTagsRead(Ok(names)) => {
                self.save_tags = Some(names);
                Task::none()
            }
            Msg::SaveTagsRead(Err(e)) => {
                self.save_tags = None;
                self.say(format!("Couldn't read the tag save: {e}"), Tone::Bad);
                Task::none()
            }

            Msg::RefreshManifest => self.fetch_manifest(),
            Msg::ManifestFetched(Ok(tags)) => {
                self.fetching_manifest = false;
                self.manifest_loaded = true;
                self.manifest = tags.into_iter().map(|t| t.0).collect();
                Task::none()
            }
            Msg::ManifestFetched(Err(e)) => {
                self.fetching_manifest = false;
                self.say(e, Tone::Bad);
                Task::none()
            }

            Msg::SearchChanged(v) => {
                self.search = v;
                Task::none()
            }
            Msg::ToggleTag(file, on) => {
                if on {
                    self.selected.insert(file);
                } else {
                    self.selected.remove(&file);
                }
                Task::none()
            }
            Msg::ClearSelection => {
                self.selected.clear();
                self.pinned.clear();
                self.misses.clear();
                self.show_misses = false;
                Task::none()
            }

            Msg::BracketUrlChanged(v) => {
                self.bracket_url = v;
                Task::none()
            }
            Msg::FindBracket => {
                if self.bracket_url.trim().is_empty() {
                    return Task::none();
                }
                if !self.manifest_loaded {
                    self.say(
                        "Still loading the tag database — try again in a moment.",
                        Tone::Warn,
                    );
                    return Task::none();
                }
                self.bracket_busy = true;
                self.misses.clear();
                self.show_misses = false;
                let url = self.bracket_url.clone();
                let manifest = self.manifest.clone();
                Task::perform(
                    async move {
                        let res = bracket::event_entrants(url).await?;
                        let m = match_bracket(&manifest, &res.entrants);
                        let uniq: HashSet<&str> =
                            res.entrants.iter().map(|e| e.slug.as_str()).collect();
                        Ok(BracketOutcome {
                            event: res.event,
                            files: m.files,
                            misses: m.misses,
                            entrant_count: uniq.len(),
                        })
                    },
                    Msg::BracketFound,
                )
            }
            Msg::BracketFound(Ok(outcome)) => {
                self.bracket_busy = false;
                self.pinned = outcome.files.iter().cloned().collect();
                self.selected.extend(outcome.files.iter().cloned());
                self.misses = outcome.misses;
                let ev = if outcome.event.is_empty() {
                    String::new()
                } else {
                    format!(" from {}", outcome.event)
                };
                if outcome.files.is_empty() {
                    self.say(
                        format!(
                            "No published tags match the {} entrant(s){ev}.",
                            outcome.entrant_count
                        ),
                        Tone::Warn,
                    );
                } else {
                    self.say(
                        format!("Selected {} tag(s){ev}.", outcome.files.len()),
                        Tone::Good,
                    );
                }
                Task::none()
            }
            Msg::BracketFound(Err(e)) => {
                self.bracket_busy = false;
                self.say(e, Tone::Bad);
                Task::none()
            }
            Msg::ToggleMisses => {
                self.show_misses = !self.show_misses;
                Task::none()
            }

            Msg::OverwriteToggled(v) => {
                self.overwrite = v;
                Task::none()
            }

            Msg::ToggleDiff(file) => {
                if self.open_diff.as_deref() == Some(file.as_str()) {
                    self.open_diff = None;
                    return Task::none();
                }
                self.open_diff = Some(file.clone());
                if self.diffs.contains_key(&file) {
                    return Task::none();
                }
                self.diffs.insert(file.clone(), DiffState::Loading);
                let save_path = self.save_path.clone();
                let dl = file.clone();
                Task::perform(
                    async move {
                        let paths = site::download_tags(vec![dl]).await?;
                        let path = paths
                            .into_iter()
                            .next()
                            .ok_or_else(|| "download failed".to_string())?;
                        blocking(move || compute_changes(&path, save_path.as_deref())).await
                    },
                    move |r| Msg::DiffReady(file.clone(), r),
                )
            }
            Msg::DiffReady(file, r) => {
                self.diffs.insert(
                    file,
                    match r {
                        Ok(ch) => DiffState::Ready(ch),
                        Err(e) => DiffState::Failed(e),
                    },
                );
                Task::none()
            }

            Msg::Install => {
                let Some(save_path) = self.save_path.clone() else {
                    self.say("Choose the tag save first.", Tone::Bad);
                    return Task::none();
                };
                if self.selected.is_empty() {
                    self.say("No tags are selected.", Tone::Bad);
                    return Task::none();
                }
                self.installing = true;
                self.say("Installing…", Tone::Plain);
                let files: Vec<String> = self.selected.iter().cloned().collect();
                // Two different people can share an in-game tag name; where we
                // know a start.gg handle, a colliding tag installs under it.
                let handles: HashMap<String, String> = self
                    .manifest
                    .iter()
                    .filter(|t| !t.startgg_tag.is_empty())
                    .map(|t| (t.file.clone(), t.startgg_tag.clone()))
                    .collect();
                let overwrite = self.overwrite;
                Task::perform(
                    async move {
                        let paths = site::download_tags(files.clone()).await?;
                        let by_path: HashMap<PathBuf, String> = paths
                            .iter()
                            .zip(files.iter())
                            .filter_map(|(p, f)| handles.get(f).map(|h| (p.clone(), h.clone())))
                            .collect();
                        install_files(save_path, paths, by_path, overwrite).await
                    },
                    Msg::Installed,
                )
            }
            Msg::PickR2tagFiles => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose .r2tag files")
                        .add_filter(".r2tag file", &["r2tag"])
                        .pick_files()
                        .await
                        .map(|fs| fs.into_iter().map(|f| f.path().to_path_buf()).collect())
                },
                Msg::R2tagsPicked,
            ),
            Msg::R2tagsPicked(None) => Task::none(),
            Msg::R2tagsPicked(Some(paths)) => {
                let Some(save_path) = self.save_path.clone() else {
                    self.say("Choose the tag save first.", Tone::Bad);
                    return Task::none();
                };
                self.installing = true;
                self.say("Installing…", Tone::Plain);
                let overwrite = self.overwrite;
                Task::perform(
                    async move { install_files(save_path, paths, HashMap::new(), overwrite).await },
                    Msg::Installed,
                )
            }
            Msg::Installed(Ok(summary)) => {
                self.installing = false;
                self.selected.clear();
                self.pinned.clear();
                self.say(summary, Tone::Good);
                self.read_save_tags()
            }
            Msg::Installed(Err(e)) => {
                self.installing = false;
                self.say(e, Tone::Bad);
                Task::none()
            }
        }
    }
}

/// The expanded per-row changes table. Mirrors the website's
/// Option Name | Old | New columns; Old is muted, New reads as the change.
fn diff_panel(state: Option<&DiffState>) -> Element<'_, Msg> {
    let muted = |s: String| text(s).size(11).color(theme::TEXT_MUTED);
    match state {
        None | Some(DiffState::Loading) => muted("computing changes…".into()).into(),
        Some(DiffState::Failed(e)) => text(format!("couldn't read this tag: {e}"))
            .size(11)
            .color(theme::TEXT_FAILURE)
            .into(),
        Some(DiffState::Ready(ch)) => {
            let mut panel = column![muted(if ch.vs_save {
                format!("vs the {} already in this save", ch.tag_name)
            } else {
                "not in this save yet — vs default settings".to_string()
            })]
            .spacing(4);

            if ch.diff.count == 0 {
                return panel
                    .push(muted(if ch.vs_save {
                        "No differences — installing changes nothing.".into()
                    } else {
                        "No differences from default settings.".into()
                    }))
                    .into();
            }

            let header = |s: &'static str| {
                text(theme::tracked(s))
                    .size(9)
                    .font(theme::FONT_BODY_SEMIBOLD)
                    .color(theme::TEXT_MUTED)
            };
            panel = panel.push(
                row![
                    header("Option").width(180),
                    header("Old").width(Length::FillPortion(1)),
                    header("New").width(Length::FillPortion(1)),
                ]
                .spacing(10),
            );
            for group in &ch.diff.groups {
                panel = panel.push(
                    text(group.scope.clone())
                        .size(11)
                        .font(theme::FONT_BODY_SEMIBOLD)
                        .color(theme::TEXT_PRIMARY),
                );
                for item in &group.items {
                    panel = panel.push(
                        row![
                            text(item.label.clone())
                                .size(11)
                                .color(theme::TEXT_PRIMARY)
                                .width(180),
                            text(item.old.clone())
                                .size(11)
                                .color(theme::TEXT_MUTED)
                                .width(Length::FillPortion(1)),
                            text(item.new.clone())
                                .size(11)
                                .color(theme::TEXT_SUCCESS)
                                .width(Length::FillPortion(1)),
                        ]
                        .spacing(10),
                    );
                }
            }
            panel.into()
        }
    }
}

/// The Option | Old | New table for one downloaded `.r2tag`: New is the
/// incoming tag, Old is the same-name tag already in the save — or, when the
/// save doesn't have one yet, the bundled default settings.
fn compute_changes(
    r2tag: &std::path::Path,
    save_path: Option<&std::path::Path>,
) -> Result<TagChanges, String> {
    let new_root = save::tag_root_json(r2tag)?;
    let tag_name = new_root
        .pointer("/properties/SavedPlayerTags_0/0")
        .and_then(|t| t.as_object())
        .and_then(|o| {
            o.iter()
                .find(|(k, _)| k.starts_with("TagName"))
                .and_then(|(_, v)| v.as_str())
        })
        .unwrap_or("")
        .to_string();

    let old_root = match save_path {
        Some(p) if !tag_name.is_empty() => save::tag_json_from_save(p, &tag_name)?,
        _ => None,
    };
    let vs_save = old_root.is_some();
    let old_digest = match &old_root {
        Some(root) => diff::extract_digest(root),
        None => diff::default_baseline(),
    };
    let new_digest = diff::extract_digest(&new_root);
    Ok(TagChanges {
        tag_name,
        vs_save,
        diff: diff::diff_digests(&new_digest, &old_digest),
    })
}

/// Preview + import a batch of `.r2tag` files into the save. Only
/// version-compatible tags are imported; the rest are counted as
/// incompatible. `handles` maps a file path to a start.gg handle used to
/// rename when two selected tags share an in-game name.
async fn install_files(
    save_path: PathBuf,
    paths: Vec<PathBuf>,
    handles: HashMap<PathBuf, String>,
    overwrite: bool,
) -> Result<String, String> {
    blocking(move || {
        let previews = save::tag_previews(&paths, &save_path)?;
        let compatible: Vec<_> = previews.iter().filter(|p| p.compatible).collect();
        let incompatible_from_preview = previews.len() - compatible.len();

        let mut by_name: HashMap<&str, Vec<&save::TagPreview>> = HashMap::new();
        for p in &compatible {
            by_name.entry(p.tag_name.as_str()).or_default().push(p);
        }
        let mut renames: HashMap<&PathBuf, &String> = HashMap::new();
        for group in by_name.values() {
            if group.len() < 2 {
                continue;
            }
            for p in group {
                if let Some(handle) = handles.get(&p.path) {
                    if handle != &p.tag_name {
                        renames.insert(&p.path, handle);
                    }
                }
            }
        }

        let instructions: Vec<save::ImportInstruction> = compatible
            .iter()
            .map(|p| save::ImportInstruction {
                path: p.path.clone(),
                tag_name: p.tag_name.clone(),
                overwrite,
                rename: renames.get(&p.path).map(|s| s.to_string()),
            })
            .collect();

        let result = save::import_tags(&save_path, instructions)?;

        let total_incompatible = result.incompatible.len() + incompatible_from_preview;
        let mut parts = Vec::new();
        if !result.imported.is_empty() {
            parts.push(format!("Installed {}", result.imported.len()));
        }
        if !result.skipped.is_empty() {
            parts.push(format!(
                "skipped {} (already in the save)",
                result.skipped.len()
            ));
        }
        if total_incompatible > 0 {
            parts.push(format!("{total_incompatible} incompatible save version"));
        }
        if parts.is_empty() {
            Ok("Nothing to install.".into())
        } else {
            Ok(parts.join(" · "))
        }
    })
    .await
}

// ---- App-level wiring --------------------------------------------------------

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    app.tag_installer.step(msg).map(Message::Tags)
}

/// Called when the screen is opened: locate the tag save (next to the stats
/// save this app already watches), read what's in it, load the manifest, and
/// prefill the bracket URL from the reporter's configured event.
pub fn opened(app: &mut App) -> Task<Message> {
    let st = &mut app.tag_installer;
    if st.save_path.is_none() {
        let stats = if !app.st.config.save.is_empty() {
            app.st.config.save.clone()
        } else {
            app.st.health.save_path.clone()
        };
        st.save_path = save::default_save_path(&stats);
    }
    if st.bracket_url.trim().is_empty() && !app.st.config.slug.is_empty() {
        st.bracket_url = app.st.config.slug.clone();
    }
    let mut tasks = vec![st.read_save_tags().map(Message::Tags)];
    if !st.manifest_loaded && !st.fetching_manifest {
        tasks.push(st.fetch_manifest().map(Message::Tags));
    }
    Task::batch(tasks)
}

// ---- screenshot seeding --------------------------------------------------------

/// Fixture state for a capture (the `tagInstaller` key of `RSR_SEED_STATE`).
/// Display-only: seeds the manifest, selection, and save summary so a shot
/// needs no network and installs nothing.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Seed {
    #[serde(default)]
    pub save_display: Option<String>,
    #[serde(default)]
    pub save_tags: Vec<String>,
    #[serde(default)]
    pub bracket_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<SeedTag>,
    #[serde(default)]
    pub misses: Vec<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Open the changes panel on the named seeded tag, with this canned
    /// Option | Old | New content.
    #[serde(default)]
    pub changes: Option<SeedChanges>,
}

/// One `[scope, [[option, old, new], …]]` seed group.
pub type SeedChangeGroup = (String, Vec<(String, String, String)>);

#[derive(Debug, Clone, Deserialize)]
pub struct SeedChanges {
    /// Which seeded tag's row to expand (by `SeedTag.name`).
    pub tag: String,
    #[serde(default)]
    pub vs_save: bool,
    /// `[scope, [[option, old, new], …]]` groups.
    #[serde(default)]
    pub groups: Vec<SeedChangeGroup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeedTag {
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub startgg_tag: String,
    /// Selected + pinned, as if a bracket Find matched it.
    #[serde(default)]
    pub matched: bool,
}

pub fn apply_seed(app: &mut App, seed: Seed) {
    let st = &mut app.tag_installer;
    if let Some(p) = seed.save_display {
        st.save_path = Some(PathBuf::from(p));
    }
    st.save_tags = Some(seed.save_tags);
    if let Some(u) = seed.bracket_url {
        st.bracket_url = u;
    }
    st.manifest_loaded = true;
    for (i, t) in seed.tags.into_iter().enumerate() {
        let file = format!("seed-{i}.r2tag.zip");
        if t.matched {
            st.selected.insert(file.clone());
            st.pinned.insert(file.clone());
        }
        st.manifest.push(site::SharedTag {
            name: t.name,
            author: t.author,
            file,
            startgg_slug: String::new(),
            startgg_tag: t.startgg_tag,
        });
    }
    st.misses = seed.misses;
    if let Some(s) = seed.status {
        st.say(s, Tone::Good);
    }
    if let Some(ch) = seed.changes {
        if let Some(t) = st.manifest.iter().find(|t| t.name == ch.tag) {
            let groups: Vec<diff::DiffGroup> = ch
                .groups
                .into_iter()
                .map(|(scope, items)| diff::DiffGroup {
                    scope,
                    items: items
                        .into_iter()
                        .map(|(label, old, new)| diff::DiffItem { label, old, new })
                        .collect(),
                })
                .collect();
            let count = groups.iter().map(|g| g.items.len()).sum();
            st.open_diff = Some(t.file.clone());
            st.diffs.insert(
                t.file.clone(),
                DiffState::Ready(TagChanges {
                    tag_name: ch.tag,
                    vs_save: ch.vs_save,
                    diff: diff::TagDiff { count, groups },
                }),
            );
        }
    }
}

// ---- view --------------------------------------------------------------------

pub fn view(app: &App) -> Element<'_, Message> {
    // Header at the app level, so it can carry the same top-right nav every
    // screen shares; everything below stays in this screen's own Msg.
    // Title and nav on one line; the tagline gets its own full-width line
    // below rather than a permanently-clipped scrap squeezed between them.
    let header = column![
        row![
            text("Tag Installer")
                .font(theme::FONT_DISPLAY)
                .size(20)
                .color(theme::TEXT_PRIMARY),
            Space::new().width(Length::Fill),
            super::nav_actions(app, super::NavView::TagInstaller),
        ]
        .spacing(10)
        .align_y(Center),
        text("everyone's saved tags on this setup, before the bracket")
            .size(12)
            .color(theme::TEXT_MUTED),
    ]
    .spacing(6);

    container(
        column![
            header,
            Element::from(screen(&app.tag_installer)).map(Message::Tags)
        ]
        .spacing(14),
    )
    .style(theme::card_rich)
    .padding(24)
    .width(Length::Fixed(920.0))
    .height(Length::Fill)
    .into()
}

fn screen(st: &State) -> Element<'_, Msg> {
    let field_label = |s: &'static str| {
        text(s)
            .size(12)
            .font(theme::FONT_BODY_MEDIUM)
            .color(theme::TEXT_MUTED)
            .width(132)
    };

    // ---- tag save -------------------------------------------------------------
    let save_label: String = match (&st.save_path, &st.save_tags) {
        (Some(p), Some(tags)) => format!("{}  ·  {} custom tag(s)", p.display(), tags.len()),
        (Some(p), None) => p.display().to_string(),
        (None, _) => "not found — has Rivals 2 been run on this PC?".into(),
    };
    // The path takes whatever room the button doesn't need and clips — a long
    // Proton-prefix path must never push Choose… off the card.
    let save_row = row![
        field_label("Tag save"),
        container(
            text(save_label)
                .size(12)
                .font(theme::FONT_MONO)
                .color(theme::TEXT_MUTED)
                .wrapping(iced::widget::text::Wrapping::None)
        )
        .width(Length::Fill)
        .clip(true),
        button(text("Choose…").size(12))
            .style(theme::button_surface)
            .padding([4, 10])
            .on_press(Msg::ChooseSave),
    ]
    .spacing(10)
    .align_y(Center);

    // ---- bracket find -----------------------------------------------------------
    let bracket_row = row![
        field_label("Everyone in a bracket"),
        text_input("https://start.gg/tournament/…/event/…", &st.bracket_url)
            .on_input(Msg::BracketUrlChanged)
            .on_submit(Msg::FindBracket)
            .style(theme::input),
        button(text(if st.bracket_busy {
            "Finding…"
        } else {
            "Find"
        }))
        .style(theme::button_primary_rich)
        .on_press_maybe(
            (!st.bracket_busy && !st.bracket_url.trim().is_empty()).then_some(Msg::FindBracket)
        ),
    ]
    .spacing(10)
    .align_y(Center);

    let mut setup = column![save_row, bracket_row].spacing(12);

    if !st.misses.is_empty() {
        let mut misses = column![button(
            text(format!(
                "{} entrant(s) without a published tag {}",
                st.misses.len(),
                if st.show_misses { "▾" } else { "▸" }
            ))
            .size(12)
        )
        .style(theme::button_linkish)
        .padding([0, 0])
        .on_press(Msg::ToggleMisses)]
        .spacing(4);
        if st.show_misses {
            misses = misses.push(text(st.misses.join(", ")).size(12).color(theme::TEXT_MUTED));
        }
        setup = setup.push(row![Space::new().width(142), misses].spacing(0));
    }

    // ---- published tag list -------------------------------------------------------
    let list_head = row![
        text(theme::tracked("Published tags"))
            .size(10)
            .font(theme::FONT_BODY_SEMIBOLD)
            .color(theme::TEXT_MUTED),
        text(if st.fetching_manifest {
            "loading…".to_string()
        } else {
            format!("{}", st.manifest.len())
        })
        .size(12)
        .color(theme::TEXT_MUTED),
        button(text("Refresh").size(12))
            .style(theme::button_linkish)
            .on_press_maybe((!st.fetching_manifest).then_some(Msg::RefreshManifest)),
        Space::new().width(Length::Fill),
        text_input("Search", &st.search)
            .on_input(Msg::SearchChanged)
            .size(13)
            .padding([4, 10])
            .width(220)
            .style(theme::input),
    ]
    .spacing(10)
    .align_y(Center);

    let visible = st.visible();
    let list: Element<'_, Msg> = if !st.manifest_loaded && st.fetching_manifest {
        container(
            text("Loading the tag database…")
                .size(13)
                .color(theme::TEXT_MUTED),
        )
        .center_x(Fill)
        .padding(24)
        .into()
    } else if visible.is_empty() {
        container(
            text(if st.search.trim().is_empty() {
                "No tags published yet.".to_string()
            } else {
                format!("Nothing matches “{}”.", st.search.trim())
            })
            .size(13)
            .color(theme::TEXT_MUTED),
        )
        .center_x(Fill)
        .padding(24)
        .into()
    } else {
        let mut rows = column![].spacing(4);
        for t in visible {
            let file = t.file.clone();
            let selected = st.selected.contains(&t.file);
            let mut r = row![
                checkbox(selected)
                    .size(16)
                    .on_toggle(move |v| Msg::ToggleTag(file.clone(), v)),
                text(t.name.clone())
                    .size(13)
                    .font(theme::FONT_BODY_BOLD)
                    .color(theme::TEXT_PRIMARY),
            ]
            .spacing(10)
            .align_y(Center);
            if !t.startgg_tag.is_empty() {
                r = r.push(
                    text(format!("@{}", t.startgg_tag))
                        .size(12)
                        .color(theme::TEXT_MUTED),
                );
            }
            if st.pinned.contains(&t.file) {
                r = r.push(text("bracket").size(11).color(theme::TEXT_SUCCESS));
            }
            r = r.push(Space::new().width(Length::Fill));
            let diff_open = st.open_diff.as_deref() == Some(t.file.as_str());
            r = r.push(
                button(
                    text(if diff_open {
                        "Hide changes"
                    } else {
                        "View changes"
                    })
                    .size(11),
                )
                .style(theme::button_linkish)
                .padding([1, 4])
                .on_press(Msg::ToggleDiff(t.file.clone())),
            );
            let mut cell = column![r].spacing(8);
            if diff_open {
                cell = cell.push(diff_panel(st.diffs.get(&t.file)));
            }
            rows = rows.push(
                container(cell)
                    .style(if selected {
                        theme::panel_live
                    } else {
                        theme::panel
                    })
                    .padding([7, 12])
                    .width(Fill),
            );
        }
        scrollable(rows).height(Fill).into()
    };

    // ---- install actions -----------------------------------------------------------
    let can_install = !st.selected.is_empty() && st.save_path.is_some() && !st.installing;
    let actions = row![
        checkbox(st.overwrite)
            .label("Overwrite existing tags")
            .text_size(13)
            .size(16)
            .on_toggle(Msg::OverwriteToggled),
        button(text("Clear selection").size(13))
            .style(theme::button_linkish)
            .on_press_maybe(
                (!st.selected.is_empty() || !st.pinned.is_empty()).then_some(Msg::ClearSelection)
            ),
        Space::new().width(Length::Fill),
        button(text("Install from .r2tag files…").size(13))
            .style(theme::button_surface)
            .on_press_maybe(
                (st.save_path.is_some() && !st.installing).then_some(Msg::PickR2tagFiles)
            ),
        button(text(if st.installing {
            "Installing…".to_string()
        } else if st.selected.is_empty() {
            "Install".to_string()
        } else {
            format!("Install {} tag(s)", st.selected.len())
        }))
        .style(theme::button_primary_rich)
        .on_press_maybe(can_install.then_some(Msg::Install)),
    ]
    .spacing(10)
    .align_y(Center);

    let status = text(&st.status)
        .size(12)
        .font(theme::FONT_BODY_MEDIUM)
        .color(match st.tone {
            Tone::Plain => theme::TEXT_MUTED,
            Tone::Good => theme::TEXT_SUCCESS,
            Tone::Warn => theme::TEXT_WARNING,
            Tone::Bad => theme::TEXT_FAILURE,
        });

    column![setup, list_head, list, actions, status]
        .spacing(14)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_tag(name: &str, file: &str, sgg: &str) -> site::SharedTag {
        site::SharedTag {
            name: name.into(),
            author: "someone".into(),
            file: file.into(),
            startgg_slug: String::new(),
            startgg_tag: sgg.into(),
        }
    }

    fn state_with_manifest() -> State {
        State {
            manifest_loaded: true,
            manifest: vec![
                manifest_tag("kim", "kim.r2tag.zip", "kimchi"),
                manifest_tag("LOOM", "loom.r2tag.zip", "loom"),
                manifest_tag("navi", "navi.r2tag.zip", ""),
            ],
            ..State::default()
        }
    }

    #[test]
    fn search_filters_by_name_handle_and_author() {
        let mut st = state_with_manifest();
        st.search = "kimchi".into();
        let names: Vec<&str> = st.visible().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["kim"]);
        st.search = "someone".into();
        assert_eq!(st.visible().len(), 3, "author matches everything");
    }

    #[test]
    fn bracket_matches_float_to_the_top_and_select() {
        let mut st = state_with_manifest();
        let _ = st.step(Msg::BracketFound(Ok(BracketOutcome {
            event: "Singles".into(),
            files: vec!["loom.r2tag.zip".into()],
            misses: vec!["PIP".into()],
            entrant_count: 2,
        })));
        assert!(st.selected.contains("loom.r2tag.zip"));
        assert_eq!(st.visible()[0].name, "LOOM", "pinned row floats to the top");
        assert_eq!(st.misses, vec!["PIP"]);
        assert!(st.status.contains("Selected 1 tag(s) from Singles"));
    }

    #[test]
    fn install_refuses_without_a_save() {
        let mut st = state_with_manifest();
        st.selected.insert("kim.r2tag.zip".into());
        let _ = st.step(Msg::Install);
        assert!(!st.installing);
        assert_eq!(st.tone, Tone::Bad);
    }

    #[test]
    fn view_renders_manifest_rows() {
        let mut st = state_with_manifest();
        st.selected.insert("kim.r2tag.zip".into());
        // The title lives in the app-level header now (with the shared nav);
        // the screen body starts at the tag-save row.
        let mut ui = iced_test::simulator(screen(&st));
        assert!(ui.find("Tag save").is_ok());
        assert!(ui.find("LOOM").is_ok(), "manifest rows should be visible");
        assert!(ui.find("Install 1 tag(s)").is_ok());
    }
}
