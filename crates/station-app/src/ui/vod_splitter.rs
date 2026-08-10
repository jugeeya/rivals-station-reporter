//! The VOD Splitter screen — the standalone startgg-vod-splitter app, folded
//! in. Turns a station's full OBS recording into one clip per set:
//!
//!   1. Fetch the event's sets from start.gg (no API token — public website
//!      endpoint). Station-measured set times are overlaid automatically
//!      from this machine's own set journals (`<out dir>/sets/set_*.json`,
//!      written once per set and never rewritten — see
//!      `vodsplit::set_files`), so cuts land where games actually started
//!      and ended rather than on start.gg's click-timestamps. A picked
//!      folder covers splitting a VOD recorded on some other machine.
//!   2. Point it at the VOD and say when the recording started.
//!   3. Build clips for your station, check the preview frames, nudge.
//!   4. Split with ffmpeg (stream copy — multi-GB VODs cut in seconds), or
//!      export the cut list for another tool.

use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;

use iced::widget::{
    button, checkbox, column, container, image, pick_list, progress_bar, row, scrollable, text,
    text_input, Space,
};
use iced::{Center, Element, Fill, Length, Task};

use chrono::TimeZone;
use serde::Deserialize;

use super::{App, Message, Screen};
use crate::theme;
use std::collections::HashMap;

use crate::vodsplit::clip::{self, Clip, Edge};
use crate::vodsplit::sets::{self, EventSets, SetInfo};
use crate::vodsplit::{ffmpeg, prefs::Prefs, set_files};

/// The ± offsets under each timecode. A real minus sign so the labels line up
/// with the "+" ones instead of sitting a pixel high on a hyphen.
const NUDGES: [(f64, &str); 6] = [
    (-60.0, "−1m"),
    (-30.0, "−30s"),
    (-5.0, "−5s"),
    (5.0, "+5s"),
    (30.0, "+30s"),
    (60.0, "+1m"),
];

/// Preview frames rendered at this width in the clip rows — a little narrower
/// than the standalone app had, to fit the reporter's 920px card.
const THUMB_DISPLAY_WIDTH: f32 = 170.0;

// ---- station picker --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationChoice {
    pub number: Option<i64>,
    pub label: String,
}

impl fmt::Display for StationChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

// ---- state -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Good,
    Warn,
    Bad,
}

/// A clip plus its (view-only) preview frames.
pub struct ClipRow {
    pub clip: Clip,
    start_thumb: Option<image::Handle>,
    end_thumb: Option<image::Handle>,
}

impl ClipRow {
    fn new(clip: Clip) -> Self {
        Self {
            clip,
            start_thumb: None,
            end_thumb: None,
        }
    }
}

pub struct State {
    config_dir: PathBuf,
    prefs: Prefs,

    slug: String,
    tournament: String,

    pub sets: Vec<SetInfo>,
    /// Custom sets folder, when splitting a VOD recorded on another machine.
    /// `None` = this app's own `<out dir>/sets`.
    sets_dir: Option<PathBuf>,
    /// This app's own sets folder, resolved from config when the screen
    /// opens (the out dir is a config field the engine owns).
    default_sets_dir: Option<PathBuf>,
    local_sets: Vec<set_files::LocalSet>,
    /// Best-effort station-set-id → start.gg-set-id links from hub state,
    /// used only to make matches exact when they happen to still exist.
    hub_links: HashMap<String, String>,
    matched: usize,
    pub stations: Vec<StationChoice>,
    pub station: Option<StationChoice>,
    fetching: bool,

    vod_path: Option<PathBuf>,
    /// Overrides the displayed path during screenshot runs.
    vod_label_override: Option<String>,
    vod_duration: Option<f64>,
    rec_start: String,

    pre: String,
    post: String,

    pub rows: Vec<ClipRow>,
    /// Thumbnails are generated one at a time; a big bracket would otherwise
    /// spawn a hundred ffmpeg processes at once and stall the machine.
    thumb_queue: VecDeque<(usize, Edge)>,
    thumbs_running: bool,

    out_dir: Option<PathBuf>,
    splitting: bool,
    split_index: usize,
    split_failures: Vec<String>,

    ffmpeg_ok: bool,
    ffmpeg_checked: bool,
    pub status: String,
    tone: Tone,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Back to the reporter (handled at the App level).
    Close,

    SlugChanged(String),
    TournamentChanged(String),
    Fetch,
    Fetched(Result<EventSets, String>),
    PickSetsDir,
    SetsDirPicked(Option<PathBuf>),
    UseAppSets,
    StationPicked(StationChoice),

    PickVod,
    VodPicked(Option<PathBuf>),
    VodProbed(Option<f64>),
    RecStartChanged(String),
    UseFileTime,
    FileTimeResolved(Option<String>),

    PreChanged(String),
    PostChanged(String),
    Build,

    ToggleClip(usize, bool),
    NameChanged(usize, String),
    TimeChanged(usize, Edge, String),
    Nudge(usize, Edge, f64),
    RemoveClip(usize),
    ClearClips,

    ThumbLoaded(usize, Edge, Option<Vec<u8>>),

    ExportCsv,
    ExportJson,
    ExportScript,
    Saved(Result<PathBuf, String>),

    PickOutDir,
    OutDirPicked(Option<PathBuf>),
    StartSplit,
    SplitStep,
    SplitFinished(Result<PathBuf, String>),

    FfmpegChecked(bool),
    SeedProbed(Option<f64>, bool),
}

impl State {
    pub fn new(config_dir: PathBuf) -> Self {
        let prefs = Prefs::load(&config_dir);
        let st = Self {
            slug: prefs.last_slug.clone(),
            tournament: prefs.tournament_name.clone(),
            sets_dir: match prefs.sets_dir.as_str() {
                "" => None,
                p => Some(PathBuf::from(p)),
            },
            prefs,
            config_dir,
            sets: Vec::new(),
            default_sets_dir: None,
            local_sets: Vec::new(),
            hub_links: HashMap::new(),
            matched: 0,
            stations: Vec::new(),
            station: None,
            fetching: false,
            vod_path: None,
            vod_label_override: None,
            vod_duration: None,
            rec_start: String::new(),
            pre: "5".into(),
            post: "8".into(),
            rows: Vec::new(),
            thumb_queue: VecDeque::new(),
            thumbs_running: false,
            out_dir: None,
            splitting: false,
            split_index: 0,
            split_failures: Vec::new(),
            ffmpeg_ok: true,
            ffmpeg_checked: false,
            status: "Fetch the event's sets, then point at the recording.".into(),
            tone: Tone::Plain,
        };
        st
    }

    /// True when no preview frames are pending — the screenshot loop waits on
    /// this so a capture never shows empty placeholders.
    pub fn thumbs_idle(&self) -> bool {
        self.thumb_queue.is_empty() && !self.thumbs_running
    }

    fn say(&mut self, msg: impl Into<String>, tone: Tone) {
        self.status = msg.into();
        self.tone = tone;
    }

    fn save_prefs(&mut self) {
        self.prefs.last_slug = self.slug.clone();
        self.prefs.tournament_name = self.tournament.clone();
        self.prefs.sets_dir = self
            .sets_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.prefs.save(&self.config_dir);
    }

    /// (Re)read the station's set journals — from the picked folder if there
    /// is one, else this app's own `<out dir>/sets` — plus any exact links
    /// hub state can still offer, and overlay onto whatever sets are
    /// currently fetched. Called on open so sets finished since the last
    /// look are picked up without any clicking.
    pub fn reload_local_sets(&mut self) {
        let dir = self
            .sets_dir
            .clone()
            .or_else(|| self.default_sets_dir.clone());
        self.local_sets = dir
            .map(|d| set_files::load_sets_dir(&d))
            .unwrap_or_default();
        self.hub_links = set_files::hub_links(&self.config_dir.join("hub-state.json"));
        self.apply_local_times();
    }

    /// Overlay station-measured times onto the fetched sets and remember how
    /// many landed, for the status line and row badges.
    fn apply_local_times(&mut self) {
        self.matched = if self.local_sets.is_empty() {
            0
        } else {
            set_files::overlay_times(&mut self.sets, &self.local_sets, &self.hub_links)
        };
    }

    fn clips(&self) -> Vec<Clip> {
        self.rows.iter().map(|r| r.clip.clone()).collect()
    }

    fn vod_name(&self) -> String {
        self.vod_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "INPUT.mkv".into())
    }

    /// Queue both preview frames for one clip, replacing any pending request
    /// for it so a burst of nudges doesn't pile up stale work.
    fn queue_thumbs(&mut self, index: usize) -> Task<Msg> {
        if self.vod_path.is_none() {
            return Task::none();
        }
        self.thumb_queue.retain(|(i, _)| *i != index);
        self.thumb_queue.push_back((index, Edge::Start));
        self.thumb_queue.push_back((index, Edge::End));
        self.pump_thumbs()
    }

    fn queue_all_thumbs(&mut self) -> Task<Msg> {
        self.thumb_queue.clear();
        for i in 0..self.rows.len() {
            self.thumb_queue.push_back((i, Edge::Start));
            self.thumb_queue.push_back((i, Edge::End));
        }
        self.pump_thumbs()
    }

    fn pump_thumbs(&mut self) -> Task<Msg> {
        if self.thumbs_running {
            return Task::none();
        }
        let Some(vod) = self.vod_path.clone() else {
            return Task::none();
        };
        let Some(&(index, edge)) = self.thumb_queue.front() else {
            return Task::none();
        };
        let Some(row) = self.rows.get(index) else {
            self.thumb_queue.pop_front();
            return self.pump_thumbs();
        };
        self.thumb_queue.pop_front();
        self.thumbs_running = true;

        // Nudge the end frame just inside the clip; seeking exactly to the end
        // often lands past the last frame and returns nothing.
        let at = match edge {
            Edge::Start => row.clip.start,
            Edge::End => (row.clip.end - 0.2).max(row.clip.start),
        };
        Task::perform(ffmpeg::thumbnail(vod, at), move |bytes| {
            Msg::ThumbLoaded(index, edge, bytes)
        })
    }

    fn rebuild_station_list(&mut self) {
        let numbers: Vec<i64> = self
            .sets
            .iter()
            .filter_map(|s| s.station)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let mut choices: Vec<StationChoice> = numbers
            .iter()
            .map(|n| {
                let count = self.sets.iter().filter(|s| s.station == Some(*n)).count();
                StationChoice {
                    number: Some(*n),
                    label: format!("Station {n} ({count} sets)"),
                }
            })
            .collect();
        choices.push(StationChoice {
            number: None,
            label: format!("All stations ({} sets)", self.sets.len()),
        });
        self.station = choices.first().cloned();
        self.stations = choices;
    }

    /// Everything except `Close`, which the App intercepts to switch screens.
    /// Runs on `&mut State` so tests can drive the screen without an `App`.
    fn step(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::Close => Task::none(),

            Msg::SlugChanged(v) => {
                self.slug = v;
                Task::none()
            }
            Msg::TournamentChanged(v) => {
                self.tournament = v;
                Task::none()
            }

            Msg::Fetch => {
                let Some(slug) = sets::parse_slug(&self.slug) else {
                    self.say(
                        "That doesn't look like a start.gg event URL or slug.",
                        Tone::Bad,
                    );
                    return Task::none();
                };
                self.save_prefs();
                self.fetching = true;
                self.say("Fetching sets…", Tone::Plain);
                Task::perform(sets::fetch_sets(slug), Msg::Fetched)
            }
            Msg::Fetched(Ok(data)) => {
                self.fetching = false;
                self.sets = data.sets;
                self.apply_local_times();
                if self.tournament.trim().is_empty() {
                    self.tournament = data.tournament_name.clone();
                }
                self.rebuild_station_list();
                if self.sets.is_empty() {
                    self.say(
                        "No sets with both a start and end time — nothing to cut from.",
                        Tone::Warn,
                    );
                } else {
                    let precise = if self.matched > 0 {
                        format!(" · {} with station-measured times", self.matched)
                    } else {
                        String::new()
                    };
                    self.say(
                        format!(
                            "{}: {} completed set(s){}. Pick your station, then build clips.",
                            if data.event_name.is_empty() {
                                "Event".into()
                            } else {
                                data.event_name
                            },
                            self.sets.len(),
                            precise,
                        ),
                        Tone::Good,
                    );
                }
                self.save_prefs();
                Task::none()
            }
            Msg::Fetched(Err(e)) => {
                self.fetching = false;
                self.say(format!("Couldn't fetch sets: {e}"), Tone::Bad);
                Task::none()
            }

            Msg::PickSetsDir => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose the recording station's set folder (…/sets)")
                        .pick_folder()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Msg::SetsDirPicked,
            ),
            Msg::SetsDirPicked(None) => Task::none(),
            Msg::SetsDirPicked(Some(dir)) => {
                self.sets_dir = Some(set_files::normalize_picked_dir(dir));
                self.reload_local_sets();
                self.save_prefs();
                if self.local_sets.is_empty() {
                    self.say(
                        "No set files in that folder — expected set_*.json \
                         (the out dir's sets/ folder).",
                        Tone::Warn,
                    );
                } else {
                    let matched = if self.sets.is_empty() {
                        "fetch the event to apply them".to_string()
                    } else {
                        format!(
                            "{} of {} fetched sets matched",
                            self.matched,
                            self.sets.len()
                        )
                    };
                    self.say(
                        format!(
                            "{} recorded set(s) found — {matched}.",
                            self.local_sets.len()
                        ),
                        Tone::Good,
                    );
                }
                Task::none()
            }
            Msg::UseAppSets => {
                self.sets_dir = None;
                self.reload_local_sets();
                self.save_prefs();
                self.say(
                    format!(
                        "Using this station's recorded sets — {} found.",
                        self.local_sets.len()
                    ),
                    Tone::Good,
                );
                Task::none()
            }
            Msg::StationPicked(choice) => {
                self.station = Some(choice);
                Task::none()
            }

            Msg::PickVod => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("Video", &["mkv", "mp4", "mov", "flv", "ts", "m4v"])
                        .pick_file()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Msg::VodPicked,
            ),
            Msg::VodPicked(None) => Task::none(),
            Msg::VodPicked(Some(path)) => {
                // OBS names recordings with the time they started, which is
                // exactly the anchor the cut math needs.
                if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) {
                    if let Some(dt) = clip::parse_obs_filename(&name) {
                        self.rec_start = clip::format_local(dt);
                        self.say("Recording start read from the filename.", Tone::Good);
                    }
                }
                self.vod_path = Some(path.clone());
                self.vod_duration = None;
                Task::perform(ffmpeg::duration(path), Msg::VodProbed)
            }
            Msg::VodProbed(dur) => {
                self.vod_duration = dur;
                if dur.is_none() && self.ffmpeg_ok {
                    self.say(
                        "Couldn't read the VOD's length (is ffprobe installed?). \
                         Clips won't be clamped to the recording.",
                        Tone::Warn,
                    );
                }
                Task::none()
            }
            Msg::RecStartChanged(v) => {
                self.rec_start = v;
                Task::none()
            }
            Msg::UseFileTime => {
                let Some(path) = self.vod_path.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let meta = tokio::fs::metadata(&path).await.ok()?;
                        let modified = meta.modified().ok()?;
                        let dt: chrono::DateTime<chrono::Local> = modified.into();
                        Some(clip::format_local(dt))
                    },
                    Msg::FileTimeResolved,
                )
            }
            Msg::FileTimeResolved(Some(s)) => {
                self.rec_start = s;
                self.say(
                    "Using the file's modified time — that's when recording *ended* \
                     for most recorders, so double-check it.",
                    Tone::Warn,
                );
                Task::none()
            }
            Msg::FileTimeResolved(None) => {
                self.say("Couldn't read the file's timestamp.", Tone::Bad);
                Task::none()
            }

            Msg::PreChanged(v) => {
                self.pre = v;
                Task::none()
            }
            Msg::PostChanged(v) => {
                self.post = v;
                Task::none()
            }

            Msg::Build => {
                if self.sets.is_empty() {
                    self.say("Fetch an event's sets first.", Tone::Bad);
                    return Task::none();
                }
                let Some(rec) = clip::parse_local_datetime(&self.rec_start) else {
                    self.say(
                        "Set the recording start time as YYYY-MM-DD HH:MM:SS.",
                        Tone::Bad,
                    );
                    return Task::none();
                };
                let pre: f64 = self.pre.trim().parse().unwrap_or(0.0);
                let post: f64 = self.post.trim().parse().unwrap_or(0.0);
                let station = self.station.as_ref().and_then(|s| s.number);

                let clips = clip::build_clips(
                    &self.sets,
                    station,
                    rec.timestamp(),
                    pre,
                    post,
                    self.vod_duration,
                    &self.tournament,
                );
                if clips.is_empty() {
                    self.say(
                        "No sets from that station fall inside this recording — \
                         check the station and the recording start time.",
                        Tone::Bad,
                    );
                    return Task::none();
                }

                let long = clips.iter().filter(|c| c.is_too_long()).count();
                self.rows = clips.into_iter().map(ClipRow::new).collect();
                if long > 0 {
                    self.say(
                        format!(
                            "Built {} clip(s), but {long} run past {} minutes — start.gg \
                             probably never got a proper end time for those. Fix the \
                             flagged rows before splitting.",
                            self.rows.len(),
                            (clip::LONG_CLIP_WARN_SECS / 60.0) as i64
                        ),
                        Tone::Warn,
                    );
                } else {
                    self.say(
                        format!(
                            "Built {} clip(s). Check the frames, then split.",
                            self.rows.len()
                        ),
                        Tone::Good,
                    );
                }
                self.save_prefs();
                self.queue_all_thumbs()
            }

            Msg::ToggleClip(i, on) => {
                if let Some(r) = self.rows.get_mut(i) {
                    r.clip.include = on;
                }
                Task::none()
            }
            Msg::NameChanged(i, name) => {
                if let Some(r) = self.rows.get_mut(i) {
                    r.clip.name = name;
                }
                Task::none()
            }
            Msg::TimeChanged(i, edge, raw) => {
                let Some(parsed) = clip::parse_clock(&raw) else {
                    return Task::none();
                };
                let dur = self.vod_duration;
                let Some(r) = self.rows.get_mut(i) else {
                    return Task::none();
                };
                match edge {
                    Edge::Start => {
                        r.clip.start = parsed.min(r.clip.end - clip::MIN_CLIP_LEN).max(0.0)
                    }
                    Edge::End => {
                        let lower = r.clip.start + clip::MIN_CLIP_LEN;
                        r.clip.end = match dur {
                            Some(d) => parsed.max(lower).min(d.max(lower)),
                            None => parsed.max(lower),
                        }
                    }
                }
                self.queue_thumbs(i)
            }
            Msg::Nudge(i, edge, delta) => {
                let dur = self.vod_duration;
                let changed = self
                    .rows
                    .get_mut(i)
                    .map(|r| clip::nudge(&mut r.clip, edge, delta, dur))
                    .unwrap_or(false);
                if changed {
                    self.queue_thumbs(i)
                } else {
                    Task::none()
                }
            }
            Msg::RemoveClip(i) => {
                if i < self.rows.len() {
                    self.rows.remove(i);
                }
                self.queue_all_thumbs()
            }
            Msg::ClearClips => {
                self.rows.clear();
                self.thumb_queue.clear();
                Task::none()
            }

            Msg::ThumbLoaded(i, edge, bytes) => {
                self.thumbs_running = false;
                if let (Some(row), Some(bytes)) = (self.rows.get_mut(i), bytes) {
                    let handle = image::Handle::from_bytes(bytes);
                    match edge {
                        Edge::Start => row.start_thumb = Some(handle),
                        Edge::End => row.end_thumb = Some(handle),
                    }
                }
                self.pump_thumbs()
            }

            Msg::ExportCsv => {
                let body = clip::export_csv(&self.clips());
                save_dialog("cuts.csv", &["csv"], body)
            }
            Msg::ExportJson => {
                let name = self.vod_name();
                let body = clip::export_json(&self.clips(), Some(&name));
                save_dialog("cuts.json", &["json"], body)
            }
            Msg::ExportScript => {
                let windows = cfg!(windows);
                let body = clip::export_script(&self.clips(), &self.vod_name(), windows);
                let (name, ext) = if windows {
                    ("split-clips.bat", "bat")
                } else {
                    ("split-clips.sh", "sh")
                };
                save_dialog(name, &[ext], body)
            }
            Msg::Saved(Ok(path)) => {
                self.say(format!("Wrote {}", path.display()), Tone::Good);
                Task::none()
            }
            Msg::Saved(Err(e)) => {
                if e.is_empty() {
                    return Task::none(); // dialog dismissed
                }
                self.say(format!("Couldn't write that file: {e}"), Tone::Bad);
                Task::none()
            }

            Msg::PickOutDir => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Msg::OutDirPicked,
            ),
            Msg::OutDirPicked(dir) => {
                if dir.is_some() {
                    self.out_dir = dir;
                }
                Task::none()
            }

            Msg::StartSplit => {
                if self.vod_path.is_none() {
                    self.say("Choose the VOD first.", Tone::Bad);
                    return Task::none();
                }
                if !self.rows.iter().any(|r| r.clip.include) {
                    self.say("No clips are ticked.", Tone::Bad);
                    return Task::none();
                }
                if self.out_dir.is_none() {
                    self.out_dir = self.vod_path.as_ref().map(|p| ffmpeg::default_out_dir(p));
                }
                self.splitting = true;
                self.split_index = 0;
                self.split_failures.clear();
                Task::done(Msg::SplitStep)
            }
            Msg::SplitStep => {
                let todo: Vec<(usize, Clip)> = self
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.clip.include)
                    .map(|(i, r)| (i, r.clip.clone()))
                    .collect();

                let Some((_, one)) = todo.get(self.split_index).cloned() else {
                    // Done.
                    self.splitting = false;
                    let total = todo.len();
                    let failed = self.split_failures.len();
                    if failed == 0 {
                        let where_to = self
                            .out_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        self.say(
                            format!("Done — {total} clip(s) written to {where_to}"),
                            Tone::Good,
                        );
                    } else {
                        self.say(
                            format!(
                                "Finished with {failed} failure(s): {}",
                                self.split_failures.join("; ")
                            ),
                            Tone::Bad,
                        );
                    }
                    return Task::none();
                };

                let vod = self.vod_path.clone().unwrap();
                let out_dir = self
                    .out_dir
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("clips"));
                let index = self.split_index;
                self.say(
                    format!("Cutting {}/{}: {}", index + 1, todo.len(), one.filename()),
                    Tone::Plain,
                );

                let filename = one.filename();
                let start = one.start;
                let len = one.len();
                Task::perform(
                    async move {
                        if let Err(e) = tokio::fs::create_dir_all(&out_dir).await {
                            return Err(format!("couldn't create output folder: {e}"));
                        }
                        ffmpeg::cut(vod, out_dir, filename, start, len).await
                    },
                    Msg::SplitFinished,
                )
            }
            Msg::SplitFinished(res) => {
                if let Err(e) = res {
                    self.split_failures.push(e);
                }
                self.split_index += 1;
                Task::done(Msg::SplitStep)
            }

            Msg::FfmpegChecked(ok) => {
                self.ffmpeg_ok = ok;
                if !ok {
                    self.say(
                        "ffmpeg isn't on your PATH — you can still build and export a \
                         cut list, but splitting here needs it installed.",
                        Tone::Warn,
                    );
                }
                Task::none()
            }
            Msg::SeedProbed(dur, build) => {
                self.vod_duration = dur;
                if build {
                    return Task::done(Msg::Build);
                }
                Task::none()
            }
        }
    }
}

// ---- App-level wiring --------------------------------------------------------

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    match msg {
        Msg::Close => {
            app.screen = Screen::Reporter;
            Task::none()
        }
        other => app.vod.step(other).map(Message::Vod),
    }
}

/// Called when the screen is opened: prefill the slug from the reporter's own
/// configured event, re-read this station's set journals (sets keep
/// finishing while this screen is closed), and check for ffmpeg once.
pub fn opened(app: &mut App) -> Task<Message> {
    if app.vod.slug.trim().is_empty() && !app.st.config.slug.is_empty() {
        app.vod.slug = app.st.config.slug.clone();
    }
    app.vod.default_sets_dir = Some(set_files::default_sets_dir(
        &app.vod.config_dir,
        &app.st.config.dir,
    ));
    app.vod.reload_local_sets();
    if !app.vod.ffmpeg_checked {
        app.vod.ffmpeg_checked = true;
        return Task::perform(ffmpeg::probe_available(), Msg::FfmpegChecked).map(Message::Vod);
    }
    Task::none()
}

// ---- screenshot seeding --------------------------------------------------------

/// Fixture state for a capture (the `vodSplitter` key of `RSR_SEED_STATE`).
/// Everything is optional so a shot can seed just the parts it cares about.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Seed {
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub tournament: Option<String>,
    #[serde(default)]
    pub vod: Option<PathBuf>,
    /// Shown in place of the real (throwaway) VOD path, so a capture never
    /// leaks the paths of whatever machine rendered it.
    #[serde(default)]
    pub vod_display: Option<String>,
    /// Recording start as a unix timestamp; the app renders it in local time,
    /// so the fixture doesn't have to guess the capturing machine's zone.
    #[serde(default)]
    pub recording_start_epoch: Option<i64>,
    #[serde(default)]
    pub station: Option<i64>,
    #[serde(default)]
    pub pre: Option<f64>,
    #[serde(default)]
    pub post: Option<f64>,
    #[serde(default)]
    pub sets: Vec<SeedSet>,
    /// Pretend this many recorded set journals were found, so the "Set
    /// times" row reads as it would mid-tournament.
    #[serde(default)]
    pub timed_sets: Option<usize>,
    /// Build the clip list straight away, so the shot shows the payoff screen.
    #[serde(default)]
    pub build: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeedSet {
    pub started_at: i64,
    pub completed_at: i64,
    #[serde(default)]
    pub station: Option<i64>,
    #[serde(default)]
    pub full_round_text: Option<String>,
    #[serde(default)]
    pub players: Vec<SeedPlayer>,
    /// Marks the set as station-timed, so the shot shows the ⏱ badge.
    #[serde(default)]
    pub precise: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeedPlayer {
    pub name: String,
    #[serde(default)]
    pub character: Option<String>,
}

impl SeedSet {
    fn into_set(self) -> SetInfo {
        SetInfo {
            id: None,
            precise: self.precise,
            started_at: self.started_at,
            completed_at: self.completed_at,
            station: self.station,
            full_round_text: self.full_round_text,
            players: self
                .players
                .into_iter()
                .map(|p| crate::vodsplit::sets::Player {
                    name: p.name,
                    character: p.character,
                })
                .collect(),
        }
    }
}

/// Preload fixture state for a screenshot run. Probing the seeded VOD is a
/// real ffprobe/ffmpeg round trip, so the shot shows genuine preview frames
/// rather than mocked ones.
pub fn apply_seed(app: &mut App, seed: Seed) -> Task<Message> {
    let st = &mut app.vod;
    if let Some(v) = seed.slug {
        st.slug = v;
    }
    if let Some(v) = seed.tournament {
        st.tournament = v;
    }
    if let Some(v) = seed.pre {
        st.pre = v.to_string();
    }
    if let Some(v) = seed.post {
        st.post = v.to_string();
    }
    if let Some(epoch) = seed.recording_start_epoch {
        if let Some(dt) = chrono::Local.timestamp_opt(epoch, 0).single() {
            st.rec_start = clip::format_local(dt);
        }
    }
    if let Some(n) = seed.timed_sets {
        st.local_sets = (0..n)
            .map(|i| set_files::LocalSet {
                set_id: format!("seed-{i}"),
                start_epoch: 1,
                end_epoch: 2,
                characters: Vec::new(),
            })
            .collect();
        st.matched = seed.sets.iter().filter(|s| s.precise).count();
    }
    if !seed.sets.is_empty() {
        st.sets = seed.sets.into_iter().map(|s| s.into_set()).collect();
        st.rebuild_station_list();
        st.say(
            format!("{} completed set(s) found.", st.sets.len()),
            Tone::Good,
        );
    }
    if let Some(number) = seed.station {
        if let Some(found) = st
            .stations
            .iter()
            .find(|s| s.number == Some(number))
            .cloned()
        {
            st.station = Some(found);
        }
    }

    st.vod_label_override = seed.vod_display;

    let task = if let Some(path) = seed.vod {
        st.vod_path = Some(path.clone());
        // Build only after the duration lands, so clips get clamped to the
        // real recording exactly as they would in normal use.
        let build = seed.build;
        Task::perform(ffmpeg::duration(path), move |dur| {
            Msg::SeedProbed(dur, build)
        })
    } else if seed.build {
        Task::done(Msg::Build)
    } else {
        Task::none()
    };
    task.map(Message::Vod)
}

// ---- view --------------------------------------------------------------------

pub fn view(app: &App) -> Element<'_, Message> {
    screen(&app.vod).map(Message::Vod)
}

fn screen(st: &State) -> Element<'_, Msg> {
    let heading = |n: &'static str, t: &'static str| {
        row![
            container(text(n).size(13).font(theme::FONT_DISPLAY))
                .center_x(24)
                .center_y(24)
                .style(theme::step_badge),
            text(t).size(17).font(theme::FONT_DISPLAY_MEDIUM),
        ]
        .spacing(10)
        .align_y(Center)
    };

    let field_label = |s: &'static str| {
        text(s)
            .size(12)
            .font(theme::FONT_BODY_MEDIUM)
            .color(theme::TEXT_MUTED)
            .width(132)
    };
    let hint = |s: String| text(s).size(12).color(theme::TEXT_MUTED);

    // ---- header ---------------------------------------------------------------
    let header = row![
        text("VOD Splitter")
            .font(theme::FONT_DISPLAY)
            .size(20)
            .color(theme::TEXT_PRIMARY),
        text("one clip per set, cut from a station's recording")
            .size(12)
            .color(theme::TEXT_MUTED),
        Space::new().width(Length::Fill),
        button(text("← Reporter").size(13))
            .style(theme::button_linkish)
            .on_press(Msg::Close),
    ]
    .spacing(10)
    .align_y(Center);

    // ---- step 1: event ----------------------------------------------------------
    let sets_label = match (&st.sets_dir, st.local_sets.len()) {
        (Some(p), n) => format!("{} — {n} recorded set(s)", p.display()),
        (None, 0) => {
            "no recorded sets on this PC yet — or choose the recording station's folder".to_string()
        }
        (None, n) => format!("this station's recorded sets — {n} found"),
    };
    let mut sets_row = row![
        field_label("Set times"),
        hint(sets_label),
        button(text("Choose folder…").size(12))
            .style(theme::button_surface)
            .padding([4, 10])
            .on_press(Msg::PickSetsDir),
    ]
    .spacing(10)
    .align_y(Center);
    if st.sets_dir.is_some() {
        sets_row = sets_row.push(
            button(text("Use this station's sets").size(12))
                .style(theme::button_linkish)
                .on_press(Msg::UseAppSets),
        );
    }

    let step1 = column![
        heading("1", "Event"),
        row![
            field_label("Event URL / slug"),
            text_input("https://start.gg/tournament/…/event/…", &st.slug)
                .on_input(Msg::SlugChanged)
                .on_submit(Msg::Fetch)
                .style(theme::input),
            button(text(if st.fetching {
                "Fetching…"
            } else {
                "Fetch sets"
            }))
            .style(theme::button_primary_rich)
            .on_press_maybe((!st.fetching).then_some(Msg::Fetch))
        ]
        .spacing(10)
        .align_y(Center),
        row![
            field_label("Tournament name"),
            text_input("shown at the start of each filename", &st.tournament)
                .on_input(Msg::TournamentChanged)
                .style(theme::input)
        ]
        .spacing(10)
        .align_y(Center),
        sets_row,
    ]
    .spacing(10);

    // ---- step 2: recording --------------------------------------------------------
    let vod_label = st
        .vod_label_override
        .clone()
        .or_else(|| st.vod_path.as_ref().map(|p| p.display().to_string()))
        .unwrap_or_else(|| "No file chosen.".into());
    let dur_label = match st.vod_duration {
        Some(d) => format!("  ·  {}", clip::clock(d)),
        None => String::new(),
    };

    let step2 = column![
        heading("2", "Recording"),
        row![
            button(text("Choose VOD…"))
                .style(theme::button_surface)
                .on_press(Msg::PickVod),
            text(format!("{vod_label}{dur_label}"))
                .size(12)
                .font(theme::FONT_MONO)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(10)
        .align_y(Center),
        row![
            field_label("Recording started"),
            text_input("YYYY-MM-DD HH:MM:SS", &st.rec_start)
                .on_input(Msg::RecStartChanged)
                .font(theme::FONT_MONO)
                .width(200)
                .style(theme::input),
            button(text("Use file time").size(13))
                .style(theme::button_surface)
                .on_press_maybe(st.vod_path.is_some().then_some(Msg::UseFileTime)),
            hint("auto-filled from OBS filenames".into()),
        ]
        .spacing(10)
        .align_y(Center),
    ]
    .spacing(10);

    // ---- step 3: clips ------------------------------------------------------------
    let step3 = column![
        heading("3", "Clips"),
        row![
            field_label("Station"),
            pick_list(
                st.stations.as_slice(),
                st.station.clone(),
                Msg::StationPicked
            )
            .placeholder("fetch an event first")
            .text_size(13)
            .style(theme::pick_list_style)
            .menu_style(theme::pick_list_menu),
            hint("Pad".into()),
            text_input("5", &st.pre)
                .on_input(Msg::PreChanged)
                .width(48)
                .style(theme::input),
            hint("s before /".into()),
            text_input("8", &st.post)
                .on_input(Msg::PostChanged)
                .width(48)
                .style(theme::input),
            hint("s after".into()),
            button(text("Build clips"))
                .style(theme::button_primary_rich)
                .on_press_maybe((!st.sets.is_empty()).then_some(Msg::Build)),
        ]
        .spacing(10)
        .align_y(Center),
    ]
    .spacing(10);

    // ---- actions / list -------------------------------------------------------------
    let has_clips = !st.rows.is_empty();
    let can_split = has_clips && st.vod_path.is_some() && !st.splitting;

    let out_label = st
        .out_dir
        .as_ref()
        .map(|p| p.display().to_string())
        // During a screenshot run the VOD is a throwaway file, so derive the
        // shown folder from the pinned display path instead of the real one.
        .or_else(|| {
            st.vod_label_override.as_ref().map(|s| {
                let parent = s.rfind(['\\', '/']).map(|i| &s[..i]).unwrap_or(s.as_str());
                format!("{parent}\\clips")
            })
        })
        .or_else(|| {
            st.vod_path
                .as_ref()
                .map(|p| ffmpeg::default_out_dir(p).display().to_string())
        })
        .unwrap_or_else(|| "a clips/ folder next to the VOD".into());

    let small = |label: &'static str, msg: Option<Msg>| {
        button(text(label).size(13))
            .style(theme::button_linkish)
            .on_press_maybe(msg)
    };

    let actions = row![
        small("Clear", has_clips.then_some(Msg::ClearClips)),
        Space::new().width(Fill),
        text(theme::tracked("Export"))
            .size(10)
            .font(theme::FONT_BODY_SEMIBOLD)
            .color(theme::TEXT_MUTED),
        small("CSV", has_clips.then_some(Msg::ExportCsv)),
        small("JSON", has_clips.then_some(Msg::ExportJson)),
        small("ffmpeg script", has_clips.then_some(Msg::ExportScript)),
        Space::new().width(14),
        button(text("Output folder…").size(13))
            .style(theme::button_surface)
            .on_press(Msg::PickOutDir),
        text(out_label)
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED),
        button(text(if st.splitting {
            "Splitting…"
        } else {
            "Split with ffmpeg"
        }))
        .style(theme::button_primary_rich)
        .on_press_maybe(can_split.then_some(Msg::StartSplit)),
    ]
    .spacing(8)
    .align_y(Center);

    let mut list = column![].spacing(8);
    for (i, r) in st.rows.iter().enumerate() {
        list = list.push(clip_row(i, r));
    }

    let body: Element<'_, Msg> = if has_clips {
        scrollable(list).height(Fill).into()
    } else {
        container(
            text("No clips yet. Fetch an event, choose the VOD, then Build clips.")
                .size(13)
                .color(theme::TEXT_MUTED),
        )
        .center_x(Fill)
        .padding(34)
        .into()
    };

    let progress: Element<'_, Msg> = if st.splitting {
        let total = st.rows.iter().filter(|r| r.clip.include).count().max(1);
        progress_bar(0.0..=1.0, st.split_index as f32 / total as f32).into()
    } else {
        Space::new().into()
    };

    let status = text(&st.status)
        .size(12)
        .font(theme::FONT_BODY_MEDIUM)
        .color(match st.tone {
            Tone::Plain => theme::TEXT_MUTED,
            Tone::Good => theme::TEXT_SUCCESS,
            Tone::Warn => theme::TEXT_WARNING,
            Tone::Bad => theme::TEXT_FAILURE,
        });

    container(column![header, step1, step2, step3, actions, progress, status, body].spacing(14))
        .style(theme::card_rich)
        .padding(24)
        .width(Length::Fixed(920.0))
        .height(Length::Fill)
        .into()
}

fn clip_row(index: usize, row_state: &ClipRow) -> Element<'_, Msg> {
    let one = &row_state.clip;

    let time_line = |edge: Edge, label: &'static str, value: f64| {
        let mut buttons = row![].spacing(3);
        for (delta, text_label) in NUDGES {
            buttons = buttons.push(
                button(text(text_label).size(11).font(theme::FONT_MONO))
                    .padding([3, 7])
                    .style(theme::button_linkish)
                    .on_press(Msg::Nudge(index, edge, delta)),
            );
        }
        column![
            row![
                text(theme::tracked(label))
                    .size(10)
                    .font(theme::FONT_BODY_SEMIBOLD)
                    .color(theme::TEXT_MUTED)
                    .width(58),
                text_input("0:00:00", &clip::clock(value))
                    .on_input(move |v| Msg::TimeChanged(index, edge, v))
                    .size(13)
                    .font(theme::FONT_MONO)
                    .padding([4, 8])
                    .width(92)
                    .style(theme::input),
            ]
            .spacing(6)
            .align_y(Center),
            buttons,
        ]
        .spacing(4)
    };

    let too_long = one.is_too_long();
    let mut len_row = row![
        text(theme::tracked("Length"))
            .size(10)
            .font(theme::FONT_BODY_SEMIBOLD)
            .color(theme::TEXT_MUTED)
            .width(58),
        text(clip::clock(one.len()))
            .size(13)
            .font(theme::FONT_MONO)
            .color(if too_long {
                theme::TEXT_FAILURE
            } else {
                theme::TEXT_PRIMARY
            }),
    ]
    .spacing(6)
    .align_y(Center);
    if too_long {
        len_row = len_row.push(
            text("⚠ unusually long — check the end time")
                .size(12)
                .font(theme::FONT_BODY_MEDIUM)
                .color(theme::TEXT_FAILURE),
        );
    }

    let fields = column![
        text_input("clip name", &one.name)
            .on_input(move |v| Msg::NameChanged(index, v))
            .size(14)
            .font(theme::FONT_BODY_MEDIUM)
            .padding([6, 10])
            .style(theme::input),
        time_line(Edge::Start, "Start", one.start),
        time_line(Edge::End, "End", one.end),
        len_row,
    ]
    .spacing(8)
    .width(Length::FillPortion(3));

    let frame = |handle: &Option<image::Handle>, caption: &'static str| {
        let inner: Element<'_, Msg> = match handle {
            Some(h) => image(h.clone()).width(THUMB_DISPLAY_WIDTH).into(),
            None => container(text("…").size(12).color(theme::TEXT_MUTED))
                .center_x(THUMB_DISPLAY_WIDTH)
                .center_y(96)
                .style(theme::thumb_placeholder)
                .into(),
        };
        column![
            inner,
            text(theme::tracked(caption))
                .size(9)
                .font(theme::FONT_BODY_SEMIBOLD)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(4)
        .align_x(Center)
    };

    let frames = row![
        frame(&row_state.start_thumb, "start"),
        frame(&row_state.end_thumb, "end"),
    ]
    .spacing(10);

    // Station-timed cuts get a quiet mark: these edges came from the
    // station's own measurements, so they rarely need nudging.
    let precise_mark: Element<'_, Msg> = if one.precise {
        text("⏱ station").size(11).color(theme::TEXT_SUCCESS).into()
    } else {
        Space::new().into()
    };

    container(
        row![
            checkbox(one.include).on_toggle(move |v| Msg::ToggleClip(index, v)),
            fields,
            precise_mark,
            frames,
            button(text("✕").size(12))
                .padding([3, 8])
                .style(theme::button_linkish)
                .on_press(Msg::RemoveClip(index)),
        ]
        .spacing(12)
        .align_y(Center),
    )
    .style(if too_long {
        theme::panel_warning
    } else {
        theme::panel
    })
    .padding(12)
    .into()
}

/// Ask where to put an exported cut list, then write it.
fn save_dialog(default_name: &str, exts: &[&str], body: String) -> Task<Msg> {
    let name = default_name.to_string();
    let exts: Vec<String> = exts.iter().map(|s| s.to_string()).collect();
    Task::perform(
        async move {
            let ext_refs: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();
            let handle = rfd::AsyncFileDialog::new()
                .set_file_name(&name)
                .add_filter("Cut list", &ext_refs)
                .save_file()
                .await;
            let Some(handle) = handle else {
                return Err(String::new()); // dismissed, not an error
            };
            let path = handle.path().to_path_buf();
            tokio::fs::write(&path, body)
                .await
                .map(|_| path)
                .map_err(|e| e.to_string())
        },
        Msg::Saved,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vodsplit::sets::Player;
    use chrono::TimeZone;

    fn scratch_state() -> State {
        let dir = std::env::temp_dir().join(format!("rsr-vodsplit-tests-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        State::new(dir)
    }

    fn state_with_sets() -> State {
        let mut st = scratch_state();
        // Two sets on station 1: one normal, one whose end time start.gg never
        // closed out (the case the long-clip warning exists for).
        st.sets = vec![
            SetInfo {
                id: None,
                precise: false,
                started_at: 1_000_100,
                completed_at: 1_000_400,
                station: Some(1),
                full_round_text: Some("Winners Final".into()),
                players: vec![
                    Player {
                        name: "jugz".into(),
                        character: Some("Fleet".into()),
                    },
                    Player {
                        name: "kim".into(),
                        character: Some("Zetter".into()),
                    },
                ],
            },
            SetInfo {
                id: None,
                precise: false,
                started_at: 1_000_500,
                completed_at: 1_004_500, // ~66 min later
                station: Some(1),
                full_round_text: Some("Grands".into()),
                players: vec![
                    Player {
                        name: "a".into(),
                        character: None,
                    },
                    Player {
                        name: "b".into(),
                        character: None,
                    },
                ],
            },
            SetInfo {
                id: None,
                precise: false,
                started_at: 1_000_100,
                completed_at: 1_000_200,
                station: Some(2),
                full_round_text: None,
                players: vec![],
            },
        ];
        st.rebuild_station_list();
        st.tournament = "Hangout".into();
        st.rec_start = clip::format_local(
            chrono::Local
                .timestamp_opt(1_000_000, 0)
                .single()
                .expect("valid timestamp"),
        );
        st
    }

    #[test]
    fn station_list_covers_each_station_plus_all() {
        let st = state_with_sets();
        let labels: Vec<&str> = st.stations.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels.len(), 3, "two stations plus an All entry");
        assert!(labels[0].starts_with("Station 1 (2 sets)"));
        assert!(labels[1].starts_with("Station 2 (1 sets)"));
        assert!(labels[2].starts_with("All stations (3 sets)"));
        // Defaults to the first real station, not "All".
        assert_eq!(st.station.as_ref().unwrap().number, Some(1));
    }

    #[test]
    fn build_makes_clips_for_the_chosen_station_and_flags_long_ones() {
        let mut st = state_with_sets();
        let _ = st.step(Msg::Build);

        assert_eq!(st.rows.len(), 2, "only station 1's sets");
        assert_eq!(
            st.rows[0].clip.name,
            "[Hangout] jugz (Fleet) vs. kim (Zetter) - Winners Final"
        );
        assert!(!st.rows[0].clip.is_too_long());
        assert!(
            st.rows[1].clip.is_too_long(),
            "66-minute set should be flagged"
        );
        assert_eq!(st.tone, Tone::Warn);
        assert!(st.status.contains("45 minutes"), "status: {}", st.status);
    }

    #[test]
    fn nudging_moves_an_edge_and_respects_clamps() {
        let mut st = state_with_sets();
        let _ = st.step(Msg::Build);
        let start_before = st.rows[0].clip.start;

        let _ = st.step(Msg::Nudge(0, Edge::Start, 30.0));
        assert_eq!(st.rows[0].clip.start, start_before + 30.0);

        // Shoving start past end pins it a minimum length short of it.
        let _ = st.step(Msg::Nudge(0, Edge::Start, 10_000.0));
        let one = &st.rows[0].clip;
        assert_eq!(one.start, one.end - clip::MIN_CLIP_LEN);
    }

    #[test]
    fn unticking_a_clip_drops_it_from_exports() {
        let mut st = state_with_sets();
        let _ = st.step(Msg::Build);
        let _ = st.step(Msg::ToggleClip(1, false));

        let csv = clip::export_csv(&st.clips());
        assert!(csv.contains("Winners Final"));
        assert!(!csv.contains("Grands"), "unticked clip should be excluded");
    }

    #[test]
    fn build_refuses_without_a_usable_recording_start() {
        let mut st = state_with_sets();
        st.rec_start = "not a time".into();
        let _ = st.step(Msg::Build);
        assert!(st.rows.is_empty());
        assert_eq!(st.tone, Tone::Bad);
    }

    #[test]
    fn station_times_overlay_on_fetch_results() {
        let mut st = state_with_sets();
        // The first set's journal: measured window inside the click-window,
        // matching characters — the fuzzy pass should claim exactly it.
        st.local_sets = vec![set_files::LocalSet {
            set_id: "20260101_010101".into(),
            start_epoch: 1_000_150,
            end_epoch: 1_000_350,
            characters: vec!["Fleet".into(), "Zetter".into()],
        }];
        st.apply_local_times();
        assert_eq!(st.matched, 1);
        assert!(st.sets[0].precise);
        assert_eq!(st.sets[0].started_at, 1_000_150);
        assert!(!st.sets[1].precise);
    }

    #[test]
    fn hub_link_makes_the_join_exact() {
        let mut st = state_with_sets();
        st.sets[0].id = Some("111".into());
        st.local_sets = vec![set_files::LocalSet {
            set_id: "20260101_010101".into(),
            start_epoch: 1_000_150,
            end_epoch: 1_000_350,
            characters: Vec::new(),
        }];
        st.hub_links.insert("20260101_010101".into(), "111".into());
        st.apply_local_times();
        assert_eq!(st.matched, 1);
        assert!(st.sets[0].precise);
    }

    #[test]
    fn view_renders_and_shows_built_clips() {
        let mut st = state_with_sets();
        let _ = st.step(Msg::Build);
        // Exercises the whole widget tree headlessly — catches view() panics
        // and proves the clip rows actually make it on screen.
        let mut ui = iced_test::simulator(screen(&st));
        assert!(ui.find("VOD Splitter").is_ok());
        assert!(
            ui.find("[Hangout] jugz (Fleet) vs. kim (Zetter) - Winners Final")
                .is_ok(),
            "the built clip should be visible in the list"
        );
    }
}
