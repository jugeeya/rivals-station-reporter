//! Current Sets — port of CurrentSets.vue: everything start.gg's bracket says
//! is happening right now, in two groups (playing now / startable), with
//! SEPARATE station and stream pickers per set (a set can carry both at
//! once). Carries over the web app's guards: picks are seeded from the
//! current assignment, re-synced while untouched, pruned when a set leaves,
//! and stale overlapping refreshes can't apply out of order.

use std::collections::HashMap;

use iced::widget::{button, column, container, pick_list, row, text, Space};
use iced::{Alignment, Element, Length, Task};

use super::{blocking, format, App, Message};
use crate::engine::commands;
use crate::model::{id_str, AvailableSet, AvailableSets};
use crate::theme;

const STARTGG_STATE_ONGOING: i64 = 2;
/// The pickers' "leave this half as it is" entry.
const NONE_STATION: &str = "no station";
const NONE_STREAM: &str = "no stream";

#[derive(Debug, Clone)]
pub enum Msg {
    Refresh,
    Loaded(u64, Result<AvailableSets, String>),
    PickStation(String, String),
    PickStream(String, String),
    Start(String),
    Change(String),
    ActionDone(Result<String, String>),
}

#[derive(Default)]
pub struct State {
    pub data: AvailableSets,
    pub loaded: bool,
    pub load_err: String,
    pub refreshing: bool,
    pub busy: Option<String>,
    pub action_msg: String,
    pub action_err: bool,
    pub picked_station: HashMap<String, String>,
    pub picked_stream: HashMap<String, String>,
    seen_station: HashMap<String, String>,
    seen_stream: HashMap<String, String>,
    gen: u64,
}

fn current_station_key(s: &AvailableSet) -> String {
    s.station
        .map(|n| n.to_string())
        .unwrap_or_else(|| NONE_STATION.to_string())
}

fn current_stream_key(s: &AvailableSet) -> String {
    s.stream.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| NONE_STREAM.to_string())
}

impl State {
    fn selection(&self, s: &AvailableSet) -> (Option<i64>, Option<String>) {
        let k = s.key();
        let st = self
            .picked_station
            .get(&k)
            .filter(|v| v.as_str() != NONE_STATION)
            .and_then(|v| v.parse::<i64>().ok());
        let sm = self
            .picked_stream
            .get(&k)
            .filter(|v| v.as_str() != NONE_STREAM)
            .cloned();
        (st, sm)
    }

    fn selection_changed(&self, s: &AvailableSet) -> bool {
        let k = s.key();
        let st = self.picked_station.get(&k);
        let sm = self.picked_stream.get(&k);
        st.is_some_and(|v| v.as_str() != NONE_STATION && *v != current_station_key(s))
            || sm.is_some_and(|v| v.as_str() != NONE_STREAM && *v != current_stream_key(s))
    }
}

pub fn refresh(app: &mut App) -> Task<Message> {
    let cs = &mut app.current_sets;
    cs.gen += 1;
    let gen = cs.gen;
    cs.refreshing = true;
    let engine = app.engine.clone();
    Task::perform(
        blocking(move || {
            commands::list_available_sets(&engine).and_then(|v| {
                serde_json::from_value::<AvailableSets>(v).map_err(|e| e.to_string())
            })
        }),
        move |r| Message::Sets(Msg::Loaded(gen, r)),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    match msg {
        Msg::Refresh => return refresh(app),
        Msg::Loaded(gen, r) => {
            let cs = &mut app.current_sets;
            // Only the newest request may write (out-of-order guard).
            if gen != cs.gen {
                return Task::none();
            }
            cs.refreshing = false;
            cs.loaded = true;
            match r {
                Ok(data) => {
                    cs.load_err.clear();
                    let live_keys: Vec<String> = data.sets.iter().map(|s| s.key()).collect();
                    for s in &data.sets {
                        let k = s.key();
                        let cur_st = current_station_key(s);
                        let cur_sm = current_stream_key(s);
                        // Seed new sets; re-sync picks the operator hasn't
                        // touched so they track changes made elsewhere.
                        let untouched_st = cs
                            .picked_station
                            .get(&k)
                            .map(|v| Some(v) == cs.seen_station.get(&k))
                            .unwrap_or(true);
                        if untouched_st {
                            cs.picked_station.insert(k.clone(), cur_st.clone());
                        }
                        let untouched_sm = cs
                            .picked_stream
                            .get(&k)
                            .map(|v| Some(v) == cs.seen_stream.get(&k))
                            .unwrap_or(true);
                        if untouched_sm {
                            cs.picked_stream.insert(k.clone(), cur_sm.clone());
                        }
                        cs.seen_station.insert(k.clone(), cur_st);
                        cs.seen_stream.insert(k, cur_sm);
                    }
                    // Prune picks for sets that left the list.
                    cs.picked_station.retain(|k, _| live_keys.contains(k));
                    cs.picked_stream.retain(|k, _| live_keys.contains(k));
                    cs.seen_station.retain(|k, _| live_keys.contains(k));
                    cs.seen_stream.retain(|k, _| live_keys.contains(k));
                    cs.data = data;
                }
                Err(e) => {
                    // Keep whatever was already listed rendering; one
                    // transient blip must not blank the panel.
                    cs.load_err = e;
                }
            }
        }
        Msg::PickStation(k, v) => {
            app.current_sets.picked_station.insert(k, v);
        }
        Msg::PickStream(k, v) => {
            app.current_sets.picked_stream.insert(k, v);
        }
        Msg::Start(key) => {
            let cs = &mut app.current_sets;
            let Some(set) = cs.data.sets.iter().find(|s| s.key() == key) else {
                return Task::none();
            };
            let (station, stream) = cs.selection(set);
            let label = set.players_label();
            cs.busy = Some(key.clone());
            cs.action_msg.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    commands::start_match(&engine, &key, station, stream)
                        .map(|_| format!("Started {label}."))
                }),
                |r| Message::Sets(Msg::ActionDone(r)),
            );
        }
        Msg::Change(key) => {
            let cs = &mut app.current_sets;
            let Some(set) = cs.data.sets.iter().find(|s| s.key() == key) else {
                return Task::none();
            };
            if !cs.selection_changed(set) {
                return Task::none();
            }
            let (station, stream) = cs.selection(set);
            let label = set.players_label();
            cs.busy = Some(key.clone());
            cs.action_msg.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    commands::reassign_destination(&engine, &key, station, stream)
                        .map(|_| format!("Moved {label}."))
                }),
                |r| Message::Sets(Msg::ActionDone(r)),
            );
        }
        Msg::ActionDone(r) => {
            let cs = &mut app.current_sets;
            cs.busy = None;
            match r {
                Ok(m) => {
                    cs.action_msg = m;
                    cs.action_err = false;
                }
                Err(e) => {
                    cs.action_msg = e;
                    cs.action_err = true;
                }
            }
            return refresh(app);
        }
    }
    Task::none()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let cs = &app.current_sets;

    let mut col = column![row![
        text("CURRENT SETS").size(11).color(theme::TEXT_MUTED),
        text(cs.data.sets.len().to_string()).size(11).color(theme::TEXT_MUTED),
        Space::new().width(Length::Fill),
        button(text(if cs.refreshing { "…" } else { "⟳" }).size(13))
            .style(theme::button_linkish)
            .padding([2, 6])
            .on_press(Message::Sets(Msg::Refresh)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)]
    .spacing(8);

    if !cs.action_msg.is_empty() {
        col = col.push(text(cs.action_msg.clone()).size(12).color(if cs.action_err {
            theme::TEXT_FAILURE
        } else {
            theme::TEXT_SUCCESS
        }));
    }

    if !cs.load_err.is_empty() && cs.data.sets.is_empty() {
        return col
            .push(text(cs.load_err.clone()).size(12).color(theme::TEXT_FAILURE))
            .into();
    }
    if !cs.load_err.is_empty() {
        col = col.push(
            text(format!("refresh failed: {}", cs.load_err))
                .size(12)
                .color(theme::TEXT_FAILURE),
        );
    }
    if cs.loaded && cs.data.sets.is_empty() {
        return col
            .push(
                text("Nothing happening on the bracket right now.")
                    .size(13)
                    .color(theme::TEXT_MUTED),
            )
            .into();
    }

    let playing: Vec<&AvailableSet> = cs
        .data
        .sets
        .iter()
        .filter(|s| s.state == Some(STARTGG_STATE_ONGOING))
        .collect();
    let startable: Vec<&AvailableSet> = cs
        .data
        .sets
        .iter()
        .filter(|s| s.state != Some(STARTGG_STATE_ONGOING))
        .collect();

    if !playing.is_empty() {
        col = col.push(
            row![
                text("●").size(9).color(theme::ACCENT),
                text("PLAYING NOW").size(10).color(theme::ACCENT),
                text(playing.len().to_string()).size(10).color(theme::TEXT_MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
        for s in playing {
            col = col.push(set_row(app, s, true));
        }
    }
    if !startable.is_empty() {
        col = col.push(
            row![
                text("STARTABLE").size(10).color(theme::TEXT_MUTED),
                text(startable.len().to_string()).size(10).color(theme::TEXT_MUTED),
            ]
            .spacing(6),
        );
        for s in startable {
            col = col.push(set_row(app, s, false));
        }
    }

    col.into()
}

fn set_row<'a>(app: &'a App, s: &'a AvailableSet, playing: bool) -> Element<'a, Message> {
    let cs = &app.current_sets;
    let k = s.key();
    let busy_here = cs.busy.as_deref() == Some(&k);

    let mut r = row![
        text(if s.full_round_text.is_empty() {
            "·".to_string()
        } else {
            s.full_round_text.clone()
        })
        .size(12)
        .color(theme::TEXT_MUTED)
        .width(Length::Fixed(150.0)),
        text(s.players_label()).size(13).color(theme::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    if playing {
        if let Some(t) = s.startgg_started_at {
            r = r.push(
                text(format::elapsed_since(t, app.now_s))
                    .font(theme::FONT_MONO)
                    .size(12)
                    .color(theme::ACCENT),
            );
        }
        if let Some(b) = format::best_of(s.startgg_total_games) {
            r = r.push(text(b).size(11).color(theme::TEXT_MUTED));
        }
    }

    // Station picker.
    let mut station_opts: Vec<String> = vec![NONE_STATION.to_string()];
    station_opts.extend(cs.data.stations.iter().map(|st| st.number.to_string()));
    let picked_st = cs
        .picked_station
        .get(&k)
        .cloned()
        .unwrap_or_else(|| current_station_key(s));
    let key_for_station = k.clone();
    r = r.push(
        pick_list(station_opts, Some(picked_st), move |v| {
            Message::Sets(Msg::PickStation(key_for_station.clone(), v))
        })
        .text_size(12)
        .padding([4, 8])
        .style(theme::pick_list_style)
        .menu_style(theme::pick_list_menu),
    );

    // Stream picker — only when the tournament has stream setups.
    if !cs.data.streams.is_empty() {
        let mut stream_opts: Vec<String> = vec![NONE_STREAM.to_string()];
        stream_opts.extend(cs.data.streams.iter().map(|st| st.name.clone()));
        let picked_sm = cs
            .picked_stream
            .get(&k)
            .cloned()
            .unwrap_or_else(|| current_stream_key(s));
        let key_for_stream = k.clone();
        r = r.push(
            pick_list(stream_opts, Some(picked_sm), move |v| {
                Message::Sets(Msg::PickStream(key_for_stream.clone(), v))
            })
            .text_size(12)
            .padding([4, 8])
            .style(theme::pick_list_style)
            .menu_style(theme::pick_list_menu),
        );
    }

    if playing {
        let mut change = button(text("Change").size(12)).style(theme::button_linkish).padding([4, 8]);
        if cs.busy.is_none() && cs.selection_changed(s) {
            change = change.on_press(Message::Sets(Msg::Change(k.clone())));
        }
        r = r.push(change);
    } else {
        let mut start = button(text(if busy_here { "…" } else { "Start Match" }).size(12))
            .style(theme::button_surface)
            .padding([5, 12]);
        if cs.busy.is_none() {
            start = start.on_press(Message::Sets(Msg::Start(k.clone())));
        }
        r = r.push(start);
    }

    container(r).padding([6, 8]).width(Length::Fill).into()
}
