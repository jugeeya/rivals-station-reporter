//! Cut math, clip naming, and cut-list exports.
//!
//! Everything here is pure so it can be unit-tested without a GUI or a VOD.

use super::sets::SetInfo;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

/// start.gg's `completedAt` is often stale or missing for a set that was never
/// properly closed out on the station, which turns into one absurd clip that
/// quietly eats the whole split. Flag those instead of letting someone find out
/// after a long encode.
pub const LONG_CLIP_WARN_SECS: f64 = 45.0 * 60.0;

/// Never let an edit collapse a clip to nothing.
pub const MIN_CLIP_LEN: f64 = 1.0;

/// A clip as the cut math sees it. Deliberately free of any UI concern (the
/// preview thumbnails hang off the view layer) so this module stays pure and
/// testable without a window.
#[derive(Debug, Clone)]
pub struct Clip {
    pub include: bool,
    pub name: String,
    /// Offsets into the VOD, in seconds.
    pub start: f64,
    pub end: f64,
    /// Cut from reporter-measured times rather than start.gg click times.
    pub precise: bool,
}

impl Clip {
    pub fn len(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    pub fn is_too_long(&self) -> bool {
        self.len() > LONG_CLIP_WARN_SECS
    }

    /// The file this clip would be written to.
    pub fn filename(&self) -> String {
        format!("{}.mp4", sanitize_filename(&self.name))
    }
}

/// `H:MM:SS`, the form the time fields use.
pub fn clock(secs: f64) -> String {
    let total = secs.max(0.0).round() as i64;
    format!(
        "{}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// Parse `H:MM:SS`, `MM:SS`, or a bare seconds count back into seconds.
pub fn parse_clock(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0f64;
    for part in s.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// Strip characters no filesystem will take, collapse runs of whitespace, and
/// cap the length so long round names can't blow past path limits.
pub fn sanitize_filename(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.chars().take(120).collect();
    let trimmed = trimmed.trim().to_string();
    if trimmed.is_empty() {
        "clip".to_string()
    } else {
        trimmed
    }
}

/// `[Tournament] P1 (Char) vs. P2 (Char) - Round`, matching the naming the
/// other builds of this tool use so clips stay consistent across them.
pub fn set_title(set: &SetInfo, tournament: &str) -> String {
    let players: Vec<String> = set
        .players
        .iter()
        .map(|p| match &p.character {
            Some(c) if !c.is_empty() => format!("{} ({})", p.name, c),
            _ => p.name.clone(),
        })
        .collect();

    let mut title = if players.is_empty() {
        "Set".to_string()
    } else {
        players.join(" vs. ")
    };
    if !tournament.trim().is_empty() {
        title = format!("[{}] {}", tournament.trim(), title);
    }
    if let Some(round) = set.full_round_text.as_ref().filter(|r| !r.is_empty()) {
        title = format!("{title} - {round}");
    }
    title
}

/// Turn the sets recorded on one station into clips, relative to when the
/// recording started. `pre`/`post` pad each side.
///
/// Sets that fall entirely outside the recording are dropped — with several
/// stations in one bracket, most sets belong to some other VOD.
pub fn build_clips(
    sets: &[SetInfo],
    station: Option<i64>,
    recording_start_epoch: i64,
    pre: f64,
    post: f64,
    vod_duration: Option<f64>,
    tournament: &str,
) -> Vec<Clip> {
    let mut clips = Vec::new();
    for set in sets {
        if let Some(want) = station {
            if set.station != Some(want) {
                continue;
            }
        }
        let start = (set.started_at - recording_start_epoch) as f64 - pre;
        let end = (set.completed_at - recording_start_epoch) as f64 + post;
        if end <= 0.0 {
            continue;
        }
        if let Some(dur) = vod_duration {
            if start >= dur {
                continue;
            }
        }
        let end = match vod_duration {
            Some(dur) => end.min(dur),
            None => end,
        };
        clips.push(Clip {
            include: true,
            name: set_title(set, tournament),
            start: start.max(0.0),
            end,
            precise: set.precise,
        });
    }
    clips
}

/// Move one edge of a clip, clamped so start can't cross end and end can't run
/// past the recording. Returns whether anything actually changed.
pub fn nudge(clip: &mut Clip, edge: Edge, delta: f64, vod_duration: Option<f64>) -> bool {
    let before = match edge {
        Edge::Start => clip.start,
        Edge::End => clip.end,
    };
    let next = match edge {
        Edge::Start => (before + delta)
            .round()
            .min(clip.end - MIN_CLIP_LEN)
            .max(0.0),
        Edge::End => {
            let lower = clip.start + MIN_CLIP_LEN;
            let raw = (before + delta).round().max(lower);
            match vod_duration {
                Some(dur) => raw.min(dur.max(lower)),
                None => raw,
            }
        }
    };
    if (next - before).abs() < f64::EPSILON {
        return false;
    }
    match edge {
        Edge::Start => clip.start = next,
        Edge::End => clip.end = next,
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

fn included(clips: &[Clip]) -> Vec<&Clip> {
    let chosen: Vec<&Clip> = clips.iter().filter(|c| c.include).collect();
    if chosen.is_empty() {
        clips.iter().collect()
    } else {
        chosen
    }
}

fn csv_cell(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// LosslessCut's CSV importer reads bare `start,end,name` rows in seconds, and
/// only skips a header when it matches its own `Start,End,Name` exactly — any
/// other wording is parsed as a cut whose times fail to parse and silently land
/// at 0. So the header has to be spelled its way. It also caps fractions at
/// three digits, hence the formatting.
pub fn export_csv(clips: &[Clip]) -> String {
    let mut out = String::from("Start,End,Name\n");
    for c in included(clips) {
        out.push_str(&format!(
            "{:.3},{:.3},{}\n",
            c.start,
            c.end,
            csv_cell(&sanitize_filename(&c.name))
        ));
    }
    out
}

pub fn export_json(clips: &[Clip], vod_name: Option<&str>) -> String {
    let cuts: Vec<serde_json::Value> = included(clips)
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "start_sec": (c.start * 1000.0).round() / 1000.0,
                "end_sec": (c.end * 1000.0).round() / 1000.0,
                "duration_sec": (c.len() * 1000.0).round() / 1000.0,
                "filename": c.filename(),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "vod": vod_name,
        "generated": Local::now().to_rfc3339(),
        "cuts": cuts,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    )
}

/// A runnable script for people who'd rather use their own ffmpeg.
pub fn export_script(clips: &[Clip], vod_name: &str, windows: bool) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
    let mut lines: Vec<String> = if windows {
        vec!["@echo off".to_string()]
    } else {
        vec!["#!/bin/sh".to_string(), "set -e".to_string()]
    };
    for c in included(clips) {
        lines.push(format!(
            "ffmpeg -y -ss {:.3} -i {} -t {:.3} -c copy -avoid_negative_ts make_zero {}",
            c.start,
            quote(vod_name),
            c.len(),
            quote(&c.filename())
        ));
    }
    lines.push("echo Done.".to_string());
    let sep = if windows { "\r\n" } else { "\n" };
    format!("{}{}", lines.join(sep), sep)
}

/// OBS's default recording filename is `YYYY-MM-DD HH-MM-SS`; pull the local
/// timestamp out of one if it looks like that.
pub fn parse_obs_filename(name: &str) -> Option<DateTime<Local>> {
    let bytes: Vec<char> = name.chars().collect();
    // Scan for the first "dddd-dd-dd" then a separator then "dd-dd-dd".
    for i in 0..bytes.len() {
        let rest: String = bytes[i..].iter().collect();
        if rest.len() < 19 {
            break;
        }
        let candidate = &rest[..19];
        let digits_ok = |s: &str, positions: &[usize]| {
            positions.iter().all(|&p| {
                s.chars()
                    .nth(p)
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
            })
        };
        if digits_ok(candidate, &[0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18])
            && candidate.chars().nth(4) == Some('-')
            && candidate.chars().nth(7) == Some('-')
            && candidate.chars().nth(13) == Some('-')
            && candidate.chars().nth(16) == Some('-')
        {
            let sep = candidate.chars().nth(10)?;
            if sep != ' ' && sep != '_' && sep != 'T' {
                continue;
            }
            let normalized = format!("{} {}", &candidate[..10], candidate[11..].replace('-', ":"));
            if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S") {
                return Local.from_local_datetime(&naive).single();
            }
        }
    }
    None
}

/// Parse the "recording start" field the user can type into.
pub fn parse_local_datetime(s: &str) -> Option<DateTime<Local>> {
    let s = s.trim();
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Local.from_local_datetime(&naive).single();
        }
    }
    None
}

pub fn format_local(dt: DateTime<Local>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vodsplit::sets::Player;

    fn clip(start: f64, end: f64, name: &str) -> Clip {
        Clip {
            include: true,
            name: name.into(),
            start,
            end,
            precise: false,
        }
    }

    #[test]
    fn clock_roundtrip() {
        assert_eq!(clock(0.0), "0:00:00");
        assert_eq!(clock(3661.0), "1:01:01");
        assert_eq!(parse_clock("1:01:01"), Some(3661.0));
        assert_eq!(parse_clock("90"), Some(90.0));
        assert_eq!(parse_clock("2:30"), Some(150.0));
        assert_eq!(parse_clock("nope"), None);
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_filename("a/b:c*d"), "a_b_c_d");
        assert_eq!(sanitize_filename("  spaced   out  "), "spaced out");
        assert_eq!(sanitize_filename("   "), "clip");
        assert_eq!(sanitize_filename(&"x".repeat(200)).chars().count(), 120);
    }

    #[test]
    fn nudge_clamps_to_bounds() {
        let mut c = clip(0.0, 60.0, "c");
        // start can't cross end
        assert!(nudge(&mut c, Edge::Start, 120.0, Some(600.0)));
        assert_eq!(c.start, 60.0 - MIN_CLIP_LEN);
        // start can't go below zero
        assert!(nudge(&mut c, Edge::Start, -600.0, Some(600.0)));
        assert_eq!(c.start, 0.0);
        // already pinned -> no change reported
        assert!(!nudge(&mut c, Edge::Start, -5.0, Some(600.0)));
        // end can't pass the VOD duration
        assert!(nudge(&mut c, Edge::End, 10_000.0, Some(600.0)));
        assert_eq!(c.end, 600.0);
    }

    #[test]
    fn flags_absurd_clips() {
        assert!(!clip(0.0, 60.0, "ok").is_too_long());
        assert!(clip(0.0, LONG_CLIP_WARN_SECS + 1.0, "bad").is_too_long());
    }

    #[test]
    fn csv_uses_the_header_losslesscut_skips() {
        let csv = export_csv(&[clip(5.0, 12.5, "A \"quoted\" name")]);
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some("Start,End,Name"));
        assert_eq!(lines.next(), Some("5.000,12.500,\"A _quoted_ name\""));
    }

    #[test]
    fn exports_only_included_clips_when_some_are_ticked() {
        let mut a = clip(0.0, 10.0, "keep");
        let mut b = clip(10.0, 20.0, "drop");
        b.include = false;
        let csv = export_csv(&[a.clone(), b.clone()]);
        assert!(csv.contains("keep"));
        assert!(!csv.contains("drop"));
        // ...but with nothing ticked, fall back to everything
        a.include = false;
        let csv = export_csv(&[a, b]);
        assert!(csv.contains("keep") && csv.contains("drop"));
    }

    #[test]
    fn builds_clips_for_one_station_within_the_recording() {
        let mk = |started: i64, completed: i64, station: i64| crate::vodsplit::sets::SetInfo {
            id: None,
            precise: false,
            started_at: started,
            completed_at: completed,
            station: Some(station),
            full_round_text: Some("Winners Final".into()),
            players: vec![
                Player {
                    name: "A".into(),
                    character: Some("Fox".into()),
                },
                Player {
                    name: "B".into(),
                    character: None,
                },
            ],
        };
        let rec = 1_000;
        let sets = vec![
            mk(1_100, 1_200, 1), // in range, station 1
            mk(1_300, 1_400, 2), // other station
            mk(100, 200, 1),     // finished before recording started
        ];
        let clips = build_clips(&sets, Some(1), rec, 5.0, 8.0, Some(600.0), "Hangout");
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].start, 95.0);
        assert_eq!(clips[0].end, 208.0);
        assert_eq!(clips[0].name, "[Hangout] A (Fox) vs. B - Winners Final");
    }

    #[test]
    fn reads_obs_filenames() {
        assert!(parse_obs_filename("2024-01-15 18-30-00.mkv").is_some());
        assert!(parse_obs_filename("station1_2024-01-15_18-30-00.mp4").is_some());
        assert!(parse_obs_filename("no-timestamp-here.mp4").is_none());
    }
}
