//! The VOD Splitter's engine — cut math, ffmpeg shell-outs, the tokenless
//! start.gg set fetcher, and the hub-state time overlay. Folded in from the
//! standalone startgg-vod-splitter app; everything UI lives in
//! `crate::ui::vod_splitter`, everything here is headless and unit-tested.

pub mod clip;
pub mod ffmpeg;
pub mod hub_times;
pub mod prefs;
pub mod sets;
