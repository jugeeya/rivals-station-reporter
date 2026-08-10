//! The operator console — port of OperatorConsole.vue + OperatorSetRow.vue.
//! Every station's sets in three groups (live / awaiting report / other),
//! with the three operator actions. Report opens a winner picker and is the
//! ONLY thing that advances the bracket.

use iced::widget::{button, column, container, row, text, tooltip, Space};
use iced::{Alignment, Element, Length, Task};
use serde_json::Value;

use super::{blocking, format, App, Message};
use crate::model::id_str;
use crate::theme;

#[derive(Debug, Clone)]
pub enum Msg {
    OpenPicker(String),
    ClosePicker,
    Report {
        station: i64,
        set_id: String,
        winner: Value,
    },
    Swap {
        station: i64,
        set_id: String,
    },
    /// First click arms; the confirming second click deletes (the native
    /// stand-in for the old confirm dialog, one fewer window).
    AskDelete(String),
    Delete {
        station: i64,
        set_id: String,
    },
    Done {
        result: Result<String, String>,
        /// True when the action changed the bracket itself (a report):
        /// Current Sets refreshes right after instead of waiting out its
        /// 20s cycle with the just-finished set still listed as playing.
        bracket_changed: bool,
    },
    ToggleOther,
}

#[derive(Default)]
pub struct State {
    pub picker_for: Option<String>,
    pub confirm_delete: Option<String>,
    pub busy: bool,
    pub action_msg: String,
    pub action_err: bool,
    pub show_other: bool,
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    let c = &mut app.console;
    match msg {
        Msg::OpenPicker(k) => {
            c.picker_for = Some(k);
            c.confirm_delete = None;
        }
        Msg::ClosePicker => c.picker_for = None,
        Msg::ToggleOther => c.show_other = !c.show_other,
        Msg::AskDelete(k) => {
            c.confirm_delete = Some(k);
            c.picker_for = None;
        }
        Msg::Report {
            station,
            set_id,
            winner,
        } => {
            c.busy = true;
            c.action_msg.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    crate::engine::commands::report_winner(&engine, station, &set_id, &winner)
                        .map(|_| "Reported to start.gg.".to_string())
                }),
                |r| {
                    Message::Console(Msg::Done {
                        result: r,
                        bracket_changed: true,
                    })
                },
            );
        }
        Msg::Swap { station, set_id } => {
            c.busy = true;
            c.action_msg.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    crate::engine::commands::swap_players(&engine, station, &set_id)
                        .map(|_| "Players switched. Remembered for future sets.".to_string())
                }),
                |r| {
                    Message::Console(Msg::Done {
                        result: r,
                        bracket_changed: false,
                    })
                },
            );
        }
        Msg::Delete { station, set_id } => {
            c.busy = true;
            c.confirm_delete = None;
            c.action_msg.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    crate::engine::commands::delete_set(&engine, station, &set_id)
                        .map(|_| "Set deleted.".to_string())
                }),
                |r| {
                    Message::Console(Msg::Done {
                        result: r,
                        bracket_changed: false,
                    })
                },
            );
        }
        Msg::Done {
            result,
            bracket_changed,
        } => {
            c.busy = false;
            c.picker_for = None;
            match result {
                Ok(m) => {
                    c.action_msg = m;
                    c.action_err = false;
                    if bracket_changed {
                        // Grace period for start.gg to settle the new set
                        // state before re-reading; the 20s cycle cleans up
                        // any eventual-consistency stragglers.
                        return Task::perform(
                            tokio::time::sleep(std::time::Duration::from_millis(800)),
                            |_| Message::SetsAutoRefresh,
                        );
                    }
                }
                Err(e) => {
                    c.action_msg = e;
                    c.action_err = true;
                }
            }
        }
    }
    Task::none()
}

fn rec_key(r: &Value) -> String {
    format!(
        "{}:{}",
        r.get("station").and_then(|v| v.as_i64()).unwrap_or(0),
        id_str(r.get("id").unwrap_or(&Value::Null))
    )
}

pub fn view(app: &App) -> Element<'_, Message> {
    let sets = &app.st.hub_snapshot.sets;
    let stations = &app.st.hub_snapshot.stations;

    let mut col = column![].spacing(10);

    // Station chips: "Stn 1 set_open" — from hubSnapshot.stations.
    if let Some(map) = stations.as_object() {
        let mut nums: Vec<_> = map.keys().cloned().collect();
        nums.sort_by_key(|k| k.parse::<i64>().unwrap_or(i64::MAX));
        if !nums.is_empty() {
            let mut chips = row![].spacing(6);
            for n in nums {
                let state = map[&n]
                    .pointer("/current/state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("—")
                    .to_string();
                // Icon instead of the raw state word; the word survives as
                // the hover text. ▶ = a set is open here, ○ = idle.
                let (glyph, color) = match state.as_str() {
                    "set_open" | "set_start" => ("▶", theme::ACCENT),
                    "idle" => ("○", theme::TEXT_MUTED),
                    _ => ("·", theme::TEXT_MUTED),
                };
                chips = chips.push(tooltip(
                    container(
                        row![
                            text(format!("Stn {n}"))
                                .font(theme::FONT_BODY_SEMIBOLD)
                                .size(11)
                                .color(theme::TEXT_PRIMARY),
                            text(glyph).size(10).color(color),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .style(theme::panel)
                    .padding([3, 8]),
                    container(text(state).size(12))
                        .style(theme::tooltip_bubble)
                        .padding(6),
                    tooltip::Position::Bottom,
                ));
            }
            col = col.push(chips);
        }
    }

    let count = sets.len();
    col = col.push(
        row![
            text(theme::tracked("All stations"))
                .font(theme::FONT_BODY_SEMIBOLD)
                .size(10)
                .color(theme::TEXT_MUTED),
            text(format!("{count} set{}", if count == 1 { "" } else { "s" }))
                .size(11)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(8),
    );

    if !app.console.action_msg.is_empty() {
        col = col.push(text(app.console.action_msg.clone()).size(12).color(
            if app.console.action_err {
                theme::TEXT_FAILURE
            } else {
                theme::TEXT_SUCCESS
            },
        ));
    }

    let status_of = |r: &Value| {
        r.get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let by_station = |a: &&Value, b: &&Value| {
        let n = |r: &Value| r.get("station").and_then(|v| v.as_i64()).unwrap_or(0);
        n(a).cmp(&n(b))
    };

    let mut live: Vec<&Value> = sets.iter().filter(|r| status_of(r) == "live").collect();
    let mut actionable: Vec<&Value> = sets.iter().filter(|r| status_of(r) == "matched").collect();
    let mut other: Vec<&Value> = sets
        .iter()
        .filter(|r| {
            let s = status_of(r);
            s != "live" && s != "matched"
        })
        .collect();
    live.sort_by(by_station);
    actionable.sort_by(by_station);
    other.sort_by(by_station);

    if !live.is_empty() {
        col = col.push(group_head("LIVE NOW", live.len(), theme::ACCENT));
        for r in live {
            col = col.push(set_row(app, r));
        }
    }
    if !actionable.is_empty() {
        col = col.push(group_head(
            "FINISHED, AWAITING REPORT",
            actionable.len(),
            theme::TEXT_WARNING,
        ));
        for r in actionable {
            col = col.push(set_row(app, r));
        }
    }
    if !other.is_empty() {
        let toggle = button(
            text(format!(
                "{}  {} {}",
                theme::tracked("Reported / not actionable"),
                other.len(),
                if app.console.show_other { "▾" } else { "▸" }
            ))
            .font(theme::FONT_BODY_SEMIBOLD)
            .size(10),
        )
        .style(theme::button_linkish)
        .padding(0)
        .on_press(Message::Console(Msg::ToggleOther));
        col = col.push(toggle);
        if app.console.show_other {
            for r in other {
                col = col.push(set_row(app, r));
            }
        }
    }

    if sets.is_empty() {
        col = col.push(
            text("No sets yet. They appear as stations report games.")
                .size(13)
                .color(theme::TEXT_MUTED),
        );
    }

    col.into()
}

fn group_head(title: &str, n: usize, color: iced::Color) -> Element<'static, Message> {
    row![
        text("●").size(9).color(color),
        text(theme::tracked(title))
            .font(theme::FONT_BODY_SEMIBOLD)
            .size(10)
            .color(color),
        text(n.to_string()).size(11).color(theme::TEXT_MUTED),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn set_row<'a>(app: &'a App, r: &'a Value) -> Element<'a, Message> {
    let key = rec_key(r);
    let station = r.get("station").and_then(|v| v.as_i64()).unwrap_or(0);
    let set_id = id_str(r.get("id").unwrap_or(&Value::Null));
    let status = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let reportable = r
        .get("reportable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let busy = app.console.busy;

    // header line: station, time, players, round, score, status badge
    let ended = r
        .pointer("/set/endEpoch")
        .and_then(|v| v.as_i64())
        .or_else(|| r.get("ingestedAt").and_then(|v| v.as_i64()));
    let started = r
        .get("startggStartedAt")
        .and_then(|v| v.as_i64())
        .or_else(|| r.pointer("/set/startEpoch").and_then(|v| v.as_i64()));

    let time_label = if status == "live" {
        started
            .map(|t| format::elapsed_since(t, app.now_s))
            .unwrap_or_default()
    } else {
        ended.map(format::clock).unwrap_or_default()
    };

    // Live rows show "12m and counting" in the accent the LIVE badge uses;
    // finished rows show the ended-at clock in muted (same as the web row).
    let time_color = if status == "live" {
        theme::ACCENT
    } else {
        theme::TEXT_MUTED
    };
    // Players line: each in-game tag with its start.gg entrant inline
    // ("BRUJITA @Brujita vs LOOM @Loom") — one place for the association
    // instead of a separate mapping section — and the WINNER's tag in green
    // once the set is decided (same rule the station views use). A set with
    // no games yet has no station-side players, so the bracket entrants
    // stand alone.
    let players_line: Element<'_, Message> = {
        let entrant_of = |slot: i64| -> Option<String> {
            r.get("slotEntrants")
                .and_then(|v| v.as_array())
                .and_then(|ss| {
                    ss.iter()
                        .find(|s| s.get("slot").and_then(|x| x.as_i64()) == Some(slot))
                })
                .and_then(|s| s.get("entrantName").and_then(|v| v.as_str()))
                .map(str::to_string)
        };
        let complete = r
            .pointer("/set/complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let winner_slot = r.pointer("/set/winnerSlot").and_then(|v| v.as_i64());
        let mut players = r
            .pointer("/set/players")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        players.sort_by_key(|p| p.get("slot").and_then(|s| s.as_i64()).unwrap_or(0));

        if players.is_empty() {
            // No games yet: the bracket entrants stand alone — styled the
            // same as the tagged path (muted "vs"), not one flat string.
            let entrants: Vec<String> = r
                .get("entrants")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect();
            if entrants.is_empty() {
                text(format::hub_players_label(r))
                    .font(theme::FONT_BODY_BOLD)
                    .size(14)
                    .color(theme::TEXT_PRIMARY)
                    .into()
            } else {
                let mut line = row![].spacing(6).align_y(Alignment::Center);
                for (i, name) in entrants.iter().enumerate() {
                    if i > 0 {
                        line = line.push(text("vs").size(11).color(theme::TEXT_MUTED));
                    }
                    line = line.push(
                        text(name.clone())
                            .font(theme::FONT_BODY_BOLD)
                            .size(14)
                            .color(theme::TEXT_PRIMARY),
                    );
                }
                line.into()
            }
        } else {
            let mut line = row![].spacing(6).align_y(Alignment::Center);
            for (i, p) in players.iter().enumerate() {
                if i > 0 {
                    line = line.push(text("vs").size(11).color(theme::TEXT_MUTED));
                }
                let slot = p.get("slot").and_then(|s| s.as_i64()).unwrap_or(0);
                let tag = p.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let won = complete && winner_slot == Some(slot);
                line = line.push(
                    text(tag.to_string())
                        .font(theme::FONT_BODY_BOLD)
                        .size(14)
                        .color(if won {
                            theme::TEXT_SUCCESS
                        } else {
                            theme::TEXT_PRIMARY
                        }),
                );
                if let Some(ent) = entrant_of(slot) {
                    line = line.push(text(format!("@{ent}")).size(11).color(theme::TEXT_MUTED));
                }
            }
            line.into()
        }
    };

    let mut head = row![
        container(
            text(station.to_string())
                .font(theme::FONT_BODY_BOLD)
                .size(13)
                .color(theme::TEXT_PRIMARY)
        )
        .style(theme::panel)
        .padding([2, 8]),
        text(time_label)
            .font(theme::FONT_MONO)
            .size(12)
            .color(time_color),
        players_line,
        Space::new().width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    if let Some(round) = r.get("fullRoundText").and_then(|v| v.as_str()) {
        head = head.push(text(round.to_string()).size(12).color(theme::TEXT_MUTED));
    }
    head = head.push(
        text(format::hub_score(r))
            .font(theme::FONT_DISPLAY)
            .size(15)
            .color(theme::TEXT_PRIMARY),
    );
    // Best-of, preferring start.gg's authoritative totalGames over the
    // station's own winsRequired guess (same rule as operatorFormat.ts).
    let best_of = r
        .get("startggTotalGames")
        .and_then(|v| v.as_i64())
        .map(|n| format!("first to {}", (n + 1) / 2))
        .or_else(|| {
            r.pointer("/set/winsRequired")
                .and_then(|v| v.as_i64())
                .map(|w| format!("first to {w}"))
        });
    if let Some(b) = best_of {
        head = head.push(text(b).size(11).color(theme::TEXT_MUTED));
    }
    // Green once start.gg's own copy of the score was read back and matched;
    // dimmed while that hasn't settled (not an error — the next push
    // re-checks). Only meaningful on live rows.
    if status == "live" {
        let confirmed = r
            .get("liveConfirmed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        head = head.push(text("✓").size(12).color(if confirmed {
            theme::TEXT_SUCCESS
        } else {
            iced::Color {
                a: 0.35,
                ..theme::TEXT_SUCCESS
            }
        }));
    }
    let badge_color = match status {
        "live" => theme::ACCENT,
        "matched" => theme::TEXT_WARNING,
        "reported" => theme::TEXT_SUCCESS,
        _ => theme::TEXT_MUTED,
    };
    // Status pill: dot + uppercase word, tinted in the status color — the
    // web row's .oc-status.
    head = head.push(
        container(
            row![
                text("●").size(7).color(badge_color),
                text(theme::tracked(status))
                    .font(theme::FONT_BODY_SEMIBOLD)
                    .size(9)
                    .color(badge_color),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .style(move |_t: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(iced::Color {
                a: 0.16,
                ..badge_color
            })),
            border: iced::Border {
                color: iced::Color {
                    a: 0.7,
                    ..badge_color
                },
                width: 1.0,
                radius: iced::border::Radius::new(999.0),
            },
            ..container::Style::default()
        })
        .padding([2, 9]),
    );

    let mut body = column![head].spacing(8);

    // Detail row: the per-game character strip on the left, the persistent
    // tag -> entrant mapping right-aligned on the right — the web row's
    // .oc-row-detail split.

    // The strip: each game its own chip, the WINNER's icon ringed in green
    // (--text-success, as the web .oc-game-side--won box-shadow) and the
    // loser dimmed, so the set's actual character history is never implied
    // by the final line-up alone.
    let games = r
        .pointer("/set/games")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let strip: Element<'_, Message> = if !games.is_empty() {
        let mut strip = row![].spacing(6).align_y(Alignment::Center);
        for g in &games {
            let num = g.get("gameNum").and_then(|v| v.as_i64()).unwrap_or(0);
            let winner = g.get("winnerSlot").and_then(|v| v.as_i64());
            let mut chip = row![text(format!("G{num}")).size(10).color(theme::TEXT_MUTED)]
                .spacing(4)
                .align_y(Alignment::Center);
            if let Some(chars) = g.get("chars").and_then(|v| v.as_array()) {
                for c in chars {
                    let slot = c.get("slot").and_then(|v| v.as_i64());
                    let ch = c.get("character").and_then(|v| v.as_str());
                    let decided = winner.is_some();
                    let won = decided && slot == winner;
                    let icon = super::chars::char_icon_opacity::<Message>(
                        ch,
                        18.0,
                        if decided && !won { 0.4 } else { 1.0 },
                    );
                    let wrapped: Element<'_, Message> = if won {
                        container(icon)
                            .style(|_t: &iced::Theme| container::Style {
                                border: iced::Border {
                                    color: theme::TEXT_SUCCESS,
                                    width: 1.5,
                                    radius: iced::border::Radius::new(10.0),
                                },
                                ..container::Style::default()
                            })
                            .padding(1)
                            .into()
                    } else {
                        container(icon).padding(1).into()
                    };
                    chip = chip.push(wrapped);
                }
            }
            strip = strip.push(container(chip).style(theme::panel).padding([2, 6]));
        }
        strip.into()
    } else {
        text("no games yet")
            .size(11)
            .color(theme::TEXT_MUTED)
            .into()
    };

    // The tag→entrant association now lives inline on the players line (one
    // "@entrant" per tag); the only thing worth a right-side note here is
    // why a grey row can't be reported.
    let mut detail = row![strip, Space::new().width(Length::Fill)]
        .spacing(12)
        .align_y(Alignment::Start);
    if !reportable {
        if let Some(reason) = r.get("notReportableReason").and_then(|v| v.as_str()) {
            detail = detail.push(text(reason.to_string()).size(11).color(theme::TEXT_MUTED));
        }
    } else if r.get("slotEntrants").and_then(|v| v.as_array()).is_none()
        && r.pointer("/set/players")
            .and_then(|v| v.as_array())
            .is_some_and(|p| !p.is_empty())
    {
        detail = detail.push(
            text("not matched to start.gg")
                .size(11)
                .color(theme::TEXT_MUTED),
        );
    }
    body = body.push(detail);

    // Winner picker (inline, replaces the action row while open).
    if app.console.picker_for.as_deref() == Some(&key) {
        let entrants = r
            .get("entrants")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let suggested = r
            .get("candidateWinnerEntrantId")
            .cloned()
            .unwrap_or(Value::Null);
        let mut pick_row = row![text("Winner:").size(12).color(theme::TEXT_MUTED)]
            .spacing(8)
            .align_y(Alignment::Center);
        for e in &entrants {
            let id = e.get("id").cloned().unwrap_or(Value::Null);
            let name = e
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let is_suggested = id_str(&id) == id_str(&suggested) && !id.is_null();
            // The name-matched candidate leads with a star and an accent
            // border — emphasized, never pre-selected (web .oc-btn--suggested).
            let label = if is_suggested {
                format!("★ {name}")
            } else {
                name
            };
            let b = button(text(label).size(13)).padding([6, 14]);
            let mut b = if is_suggested {
                b.style(|t: &iced::Theme, s| {
                    let mut style = theme::button_surface(t, s);
                    style.border.color = theme::ACCENT;
                    style
                })
            } else {
                b.style(theme::button_surface)
            };
            if !busy {
                b = b.on_press(Message::Console(Msg::Report {
                    station,
                    set_id: set_id.clone(),
                    winner: id,
                }));
            }
            pick_row = pick_row.push(b);
        }
        pick_row = pick_row.push(
            button(text("cancel").size(12))
                .style(theme::button_linkish)
                .on_press(Message::Console(Msg::ClosePicker)),
        );
        body = body.push(pick_row);
    } else {
        // Action row.
        let mut actions = row![].spacing(8).align_y(Alignment::Center);
        if reportable && status != "reported" {
            let mut b = button(text("Report").size(13))
                .style(theme::button_primary_rich)
                .padding([6, 16]);
            if !busy {
                b = b.on_press(Message::Console(Msg::OpenPicker(key.clone())));
            }
            actions = actions.push(b);
        } else if status == "reported" {
            let mut b = button(text("Re-report").size(12))
                .style(theme::button_surface)
                .padding([5, 12]);
            if !busy {
                b = b.on_press(Message::Console(Msg::OpenPicker(key.clone())));
            }
            actions = actions.push(b);
        }
        let mut swap = button(text("⇄ switch players").size(12))
            .style(theme::button_linkish)
            .padding([5, 8]);
        if !busy {
            swap = swap.on_press(Message::Console(Msg::Swap {
                station,
                set_id: set_id.clone(),
            }));
        }
        actions = actions.push(tooltip(
            swap,
            container(
                text("The station guessed who's who backwards. Flip it (characters and live score follow)")
                    .size(12),
            )
            .style(theme::tooltip_bubble)
            .padding(8)
            .max_width(320),
            tooltip::Position::Top,
        ));

        if app.console.confirm_delete.as_deref() == Some(&key) {
            let mut confirm = button(text("really delete?").size(12))
                .style(theme::button_surface)
                .padding([5, 10]);
            if !busy {
                confirm = confirm.on_press(Message::Console(Msg::Delete {
                    station,
                    set_id: set_id.clone(),
                }));
            }
            actions = actions.push(confirm);
        } else {
            let mut del = button(text("✕").size(13))
                .style(theme::button_linkish)
                .padding([5, 8]);
            if !busy {
                del = del.on_press(Message::Console(Msg::AskDelete(key.clone())));
            }
            actions = actions.push(tooltip(
                del,
                container(
                    text("Delete this set from the console (start.gg is untouched)").size(12),
                )
                .style(theme::tooltip_bubble)
                .padding(8)
                .max_width(320),
                tooltip::Position::Top,
            ));
        }
        body = body.push(actions);
    }

    // Row chrome carries the status: an accent/warning edge + faint tint for
    // live/awaiting rows (the web row's border-left + color-mix background),
    // reported rows visually receded.
    let (edge, tint): (Option<iced::Color>, f32) = match status {
        "live" => (Some(theme::ACCENT), 0.07),
        "matched" => (Some(theme::TEXT_WARNING), 0.06),
        _ => (None, 0.0),
    };
    let styled = container(match edge {
        Some(color) => {
            let rule = container(Space::new())
                .width(Length::Fixed(3.0))
                .height(Length::Fill)
                .style(move |_t: &iced::Theme| container::Style {
                    background: Some(iced::Background::Color(color)),
                    border: iced::Border {
                        radius: iced::border::Radius::new(2.0),
                        ..Default::default()
                    },
                    ..container::Style::default()
                });
            row![rule, body].spacing(10).into()
        }
        None => Element::<'_, Message>::from(body),
    })
    .style(move |t: &iced::Theme| {
        let mut s = theme::panel(t);
        if let Some(color) = edge {
            s.background = Some(iced::Background::Color(iced::Color { a: tint, ..color }));
        }
        if status == "reported" {
            s.text_color = Some(theme::TEXT_MUTED);
        }
        s
    })
    .padding(12)
    .width(Length::Fill);

    styled.into()
}
