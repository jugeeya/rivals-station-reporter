//! The operator console — port of OperatorConsole.vue + OperatorSetRow.vue.
//! Every station's sets in three groups (live / awaiting report / other),
//! with the operator actions. Report opens a winner picker.
//!
//! Report used to be the only thing here that advanced a bracket. With
//! auto-report on it isn't: an unambiguous finished set finalizes itself as
//! soon as it ends (see `station_core::hub::auto_report_blocker` for what
//! qualifies). There is no window to catch that in, by design — so the row
//! says who reported it, and `edit result` stays available afterwards, which
//! re-reports over what start.gg already has.

use iced::widget::{button, column, container, pick_list, row, text, tooltip, Space};
use iced::{Alignment, Element, Length, Task};
use serde_json::Value;

use super::{blocking, format, App, Message, Screen};
use crate::model::id_str;
use crate::theme;

/// Upper bound on games in the editor. Matches the hub's own limit, so Save
/// can't be refused for a length the editor happily let you build.
const MAX_EDIT_GAMES: usize = station_core::hub::MAX_GAMES_PER_SET;

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
    /// Report over a result start.gg already has (after correcting it).
    ReReport {
        station: i64,
        set_id: String,
        winner: Value,
    },
    /// Open the result editor on this row, seeded from what the station saw.
    OpenEditor(String, Vec<EditGame>),
    CloseEditor,
    /// Flip which slot won game `index`.
    EditWinner(usize, i64),
    /// Set the character slot `slot` played in game `index`.
    EditChar(usize, usize, String),
    EditAddGame,
    EditRemoveGame(usize),
    SaveEdit {
        station: i64,
        set_id: String,
        /// The set already reached the bracket, so saving has to push the
        /// correction over it rather than just storing it.
        rereport: bool,
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

/// One game as the editor holds it: who won, and what each slot played.
/// Characters are full names ("Zetterburn"), the same form the rest of the
/// app uses — `station_core::stats::char_full` has already been applied by
/// the time a game reaches a record.
#[derive(Debug, Clone, PartialEq)]
pub struct EditGame {
    pub winner_slot: i64,
    pub chars: [String; 2],
}

#[derive(Default)]
pub struct State {
    pub picker_for: Option<String>,
    pub confirm_delete: Option<String>,
    pub busy: bool,
    pub action_msg: String,
    pub action_err: bool,
    pub show_other: bool,
    /// Which row has the result editor open, and its working copy. Nothing
    /// reaches the hub until Save, so an abandoned edit changes nothing.
    pub editing: Option<(String, Vec<EditGame>)>,
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    // Handled before the console borrow: a finished action may need to
    // refresh the Bracket screen too, which borrows all of `app`.
    if let Msg::Done {
        result,
        bracket_changed,
    } = msg
    {
        let c = &mut app.console;
        c.busy = false;
        c.picker_for = None;
        if result.is_ok() {
            c.editing = None;
        }
        match result {
            Ok(m) => {
                c.action_msg = m;
                c.action_err = false;
                if bracket_changed {
                    // Grace period for start.gg to settle the new set
                    // state before re-reading; the 20s cycle cleans up
                    // any eventual-consistency stragglers.
                    let sets = Task::perform(
                        tokio::time::sleep(std::time::Duration::from_millis(800)),
                        |_| Message::SetsAutoRefresh,
                    );
                    // The same set sits in the Bracket screen's tree; a
                    // report made from its station card must reach it too.
                    if app.screen == Screen::Bracket {
                        return Task::batch([sets, super::bracket::refresh(app)]);
                    }
                    return sets;
                }
            }
            Err(e) => {
                c.action_msg = e;
                c.action_err = true;
            }
        }
        return Task::none();
    }

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
        Msg::ReReport {
            station,
            set_id,
            winner,
        } => {
            c.busy = true;
            c.action_msg.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    crate::engine::commands::rereport_winner(&engine, station, &set_id, &winner)
                        .map(|_| "Re-reported to start.gg.".to_string())
                }),
                |r| {
                    Message::Console(Msg::Done {
                        result: r,
                        bracket_changed: true,
                    })
                },
            );
        }
        Msg::OpenEditor(key, games) => {
            c.editing = Some((key, games));
            c.picker_for = None;
            c.confirm_delete = None;
        }
        Msg::CloseEditor => c.editing = None,
        Msg::EditWinner(i, slot) => {
            if let Some((_, games)) = &mut c.editing {
                if let Some(g) = games.get_mut(i) {
                    g.winner_slot = slot;
                }
            }
        }
        Msg::EditChar(i, slot, character) => {
            if let Some((_, games)) = &mut c.editing {
                if let Some(g) = games.get_mut(i) {
                    g.chars[slot] = character;
                }
            }
        }
        Msg::EditAddGame => {
            if let Some((_, games)) = &mut c.editing {
                if games.len() < MAX_EDIT_GAMES {
                    // A new game inherits the last one's characters: adding
                    // game 4 to a set usually means more of the same matchup,
                    // and a wrong carried-over pick is one click to fix.
                    let chars = games
                        .last()
                        .map(|g| g.chars.clone())
                        .unwrap_or_else(|| [String::new(), String::new()]);
                    games.push(EditGame {
                        winner_slot: 0,
                        chars,
                    });
                }
            }
        }
        Msg::EditRemoveGame(i) => {
            if let Some((_, games)) = &mut c.editing {
                if games.len() > 1 && i < games.len() {
                    games.remove(i);
                }
            }
        }
        Msg::SaveEdit {
            station,
            set_id,
            rereport,
        } => {
            let Some((_, games)) = c.editing.clone() else {
                return Task::none();
            };
            c.busy = true;
            c.action_msg.clear();
            let payload = Value::Array(
                games
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "winnerSlot": g.winner_slot,
                            "chars": [
                                {"slot": 0, "character": g.chars[0].clone()},
                                {"slot": 1, "character": g.chars[1].clone()},
                            ],
                        })
                    })
                    .collect::<Vec<_>>(),
            );
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    let rec = crate::engine::commands::override_result(
                        &engine, station, &set_id, &payload,
                    )?;
                    if !rereport {
                        return Ok("Result corrected.".to_string());
                    }
                    // The bracket already has the old result, so the
                    // correction is only real once start.gg has it too.
                    let winner = rec
                        .get("candidateWinnerEntrantId")
                        .cloned()
                        .unwrap_or(Value::Null);
                    crate::engine::commands::rereport_winner(&engine, station, &set_id, &winner)
                        .map(|_| "Result corrected and re-reported.".to_string())
                }),
                move |r| {
                    Message::Console(Msg::Done {
                        result: r,
                        bracket_changed: rereport,
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
        // Handled above, before the console borrow.
        Msg::Done { .. } => unreachable!("handled at the top of update"),
    }
    Task::none()
}

/// Character names for the editor's pickers, from the one canonical table
/// (`station_core::stats::CHARACTERS`), so a roster addition reaches this
/// screen by updating that table and nothing else. A blank entry stays first:
/// "no character recorded" is a real state, and forcing a guess would write
/// one into start.gg's game data.
fn character_options() -> Vec<String> {
    let mut out = vec![String::new()];
    out.extend(
        station_core::stats::CHARACTERS
            .iter()
            .filter(|(_, full)| *full != "Random")
            .map(|(_, full)| (*full).to_string()),
    );
    out
}

/// Seed the editor from what the station recorded, so correcting a set starts
/// from its actual result rather than a blank slate. A set with no usable
/// games at all opens with one, since the editor's whole job is entering the
/// games the station missed.
fn games_from_record(r: &Value) -> Vec<EditGame> {
    let games: Vec<EditGame> = r
        .pointer("/set/games")
        .and_then(|v| v.as_array())
        .map(|gs| {
            gs.iter()
                .map(|g| {
                    let ch = |slot: i64| -> String {
                        g.get("chars")
                            .and_then(|v| v.as_array())
                            .and_then(|a| {
                                a.iter()
                                    .find(|c| c.get("slot").and_then(|v| v.as_i64()) == Some(slot))
                            })
                            .and_then(|c| c.get("character"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string()
                    };
                    EditGame {
                        winner_slot: g.get("winnerSlot").and_then(|v| v.as_i64()).unwrap_or(0),
                        chars: [ch(0), ch(1)],
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if games.is_empty() {
        vec![EditGame {
            winner_slot: 0,
            chars: [String::new(), String::new()],
        }]
    } else {
        games
    }
}

/// The result editor: one line per game, and everything else (score, winner,
/// game count) derived from those lines. Editing the games rather than the
/// score directly is what keeps the three from ever disagreeing — which is
/// the failure this exists to prevent, since all three go to start.gg.
fn result_editor<'a>(
    games: &'a [EditGame],
    names: [String; 2],
    station: i64,
    set_id: &str,
    busy: bool,
    // `reported`: the set already reached the bracket, so saving has to push
    // the correction over it — and the button should say so.
    reported: bool,
) -> Element<'a, Message> {
    let wins = |slot: i64| games.iter().filter(|g| g.winner_slot == slot).count();

    let mut col = column![row![
        text("Correct the result")
            .size(12)
            .font(theme::FONT_BODY_SEMIBOLD)
            .color(theme::TEXT_PRIMARY),
        text(format!("{} {}–{} {}", names[0], wins(0), wins(1), names[1]))
            .size(12)
            .color(theme::TEXT_MUTED),
    ]
    .spacing(10)
    .align_y(Alignment::Center)]
    .spacing(6);

    let options = character_options();
    for (i, g) in games.iter().enumerate() {
        let mut line = row![text(format!("G{}", i + 1))
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED)
            .width(Length::Fixed(24.0))]
        .spacing(6)
        .align_y(Alignment::Center);

        // Who won: the two tags, the winner lit. Buttons rather than a
        // picker — two options, and the answer has to be readable at a glance.
        for slot in 0..2i64 {
            let won = g.winner_slot == slot;
            let mut b = button(text(names[slot as usize].clone()).size(12))
                .style(if won {
                    theme::button_primary_rich
                } else {
                    theme::button_surface
                })
                .padding([4, 10]);
            if !busy && !won {
                b = b.on_press(Message::Console(Msg::EditWinner(i, slot)));
            }
            line = line.push(b);
        }

        for slot in 0..2usize {
            let current = g.chars[slot].clone();
            line = line.push(
                pick_list(options.clone(), Some(current), move |c| {
                    Message::Console(Msg::EditChar(i, slot, c))
                })
                .placeholder("character")
                .text_size(11)
                .padding([3, 6])
                .width(Length::Fixed(112.0))
                .style(theme::pick_list_style)
                .menu_style(theme::pick_list_menu),
            );
        }

        let mut rm = button(text("✕").size(11))
            .style(theme::button_linkish)
            .padding([3, 6]);
        if !busy && games.len() > 1 {
            rm = rm.on_press(Message::Console(Msg::EditRemoveGame(i)));
        }
        line = line.push(rm);
        col = col.push(line);
    }

    let mut add = button(text("+ game").size(12))
        .style(theme::button_linkish)
        .padding([4, 8]);
    if !busy && games.len() < MAX_EDIT_GAMES {
        add = add.on_press(Message::Console(Msg::EditAddGame));
    }
    let mut save = button(
        text(if reported {
            "Save & re-report"
        } else {
            "Save result"
        })
        .size(12),
    )
    .style(theme::button_primary_rich)
    .padding([5, 14]);
    // A drawn correction is a mistake, not a result: the hub would store it
    // with no winner and keep it off the bracket, so refuse it here where
    // there is somewhere to say why.
    let drawn = wins(0) == wins(1);
    if !busy && !drawn {
        save = save.on_press(Message::Console(Msg::SaveEdit {
            station,
            set_id: set_id.to_string(),
            rereport: reported,
        }));
    }
    let mut foot = row![
        add,
        Space::new().width(Length::Fill),
        save,
        button(text("cancel").size(12))
            .style(theme::button_linkish)
            .padding([5, 8])
            .on_press(Message::Console(Msg::CloseEditor)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if drawn {
        foot = foot.push(
            text("a set needs a winner")
                .size(11)
                .color(theme::TEXT_WARNING),
        );
    }
    col = col.push(foot);

    container(col)
        .style(theme::panel)
        .padding(10)
        .width(Length::Fill)
        .into()
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

/// One station record as a full card: tags with their entrants, the per-game
/// character strip, score, status, and the report / edit result / switch
/// players actions. `pub(super)` because the Bracket screen renders the same
/// card under a selected set one of the hub's stations tracked.
pub(super) fn set_row<'a>(app: &'a App, r: &'a Value) -> Element<'a, Message> {
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

    // Result editor (inline, replaces the action row while open).
    if let Some((editing_key, games)) = app.console.editing.as_ref().filter(|(k, _)| *k == key) {
        let _ = editing_key;
        // The in-game player names, which is what the operator is looking at
        // on the setup — not the bracket entrant names, which may differ.
        let name_of = |slot: i64| -> String {
            r.pointer("/set/players")
                .and_then(|v| v.as_array())
                .and_then(|ps| {
                    ps.iter()
                        .find(|p| p.get("slot").and_then(|v| v.as_i64()) == Some(slot))
                })
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(if slot == 0 { "P1" } else { "P2" })
                .to_string()
        };
        body = body.push(result_editor(
            games,
            [name_of(0), name_of(1)],
            station,
            &set_id,
            busy,
            status == "reported",
        ));
    }
    // Winner picker (inline, replaces the action row while open).
    else if app.console.picker_for.as_deref() == Some(&key) {
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
            // Who decided. Worth saying: a result nobody clicked for is
            // exactly the one an operator will want to double-check.
            if r.get("reportedBy").and_then(|v| v.as_str()) == Some("auto") {
                actions = actions.push(
                    text("reported automatically")
                        .size(12)
                        .color(theme::TEXT_MUTED),
                );
            }
        }
        // An auto-report that gave up says why, rather than leaving the set
        // looking like it's still counting down to something.
        if let Some(e) = r.get("autoReportError").and_then(|v| v.as_str()) {
            if status != "reported" {
                actions = actions.push(
                    text(format!("auto-report stopped: {e}"))
                        .size(12)
                        .color(theme::TEXT_FAILURE),
                );
            }
        }
        // Correcting the result is what makes immediate auto-report
        // reasonable: the station reads the save file, which is right almost
        // always and wrong in ways it can't detect. Offered on REPORTED rows
        // too — with no window before a set goes out, after the fact is the
        // only time there is to catch one.
        {
            let mut edit = button(text("✎ edit result").size(12))
                .style(theme::button_linkish)
                .padding([5, 8]);
            if !busy {
                edit = edit.on_press(Message::Console(Msg::OpenEditor(
                    key.clone(),
                    games_from_record(r),
                )));
            }
            actions = actions.push(tooltip(
                edit,
                container(
                    text(if status == "reported" {
                        "Fix a result that already went out. Saving re-reports it, resetting the set on start.gg first."
                    } else {
                        "Set the games, characters and score by hand. What you enter is what start.gg gets."
                    })
                    .size(12),
                )
                .style(theme::tooltip_bubble)
                .padding(8)
                .max_width(320),
                tooltip::Position::Top,
            ));
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
    let styled = container(body)
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
        .padding(iced::Padding {
            top: 12.0,
            right: 12.0,
            bottom: 12.0,
            // Room for the edge overlay (3px strip + the old 10px gap).
            left: if edge.is_some() { 25.0 } else { 12.0 },
        })
        .width(Length::Fill);

    // The edge is an overlay in a Shrink stack, whose base child (the card)
    // dictates the height — never a Fill-height rule inside the row, which
    // would make the card grab all free height when it renders outside a
    // scrollable (the Bracket screen puts it in a plain column).
    match edge {
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
            iced::widget::stack![
                styled,
                container(rule).padding(iced::Padding {
                    top: 12.0,
                    right: 0.0,
                    bottom: 12.0,
                    left: 12.0,
                })
            ]
            .into()
        }
        None => styled.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn games(n: usize) -> Vec<EditGame> {
        (0..n)
            .map(|i| EditGame {
                winner_slot: (i % 2) as i64,
                chars: ["Orcane".into(), "Galvan".into()],
            })
            .collect()
    }

    #[test]
    fn the_editor_seeds_from_what_the_station_recorded() {
        // Correcting a set starts from its actual result — retyping four
        // games to fix the fifth would be its own source of mistakes.
        let rec = json!({"set": {"games": [
            {"gameNum": 1, "winnerSlot": 0,
             "chars": [{"slot": 0, "character": "Orcane"}, {"slot": 1, "character": "Galvan"}]},
            {"gameNum": 2, "winnerSlot": 1,
             "chars": [{"slot": 0, "character": "Orcane"}, {"slot": 1, "character": "Zetterburn"}]},
        ]}});
        let got = games_from_record(&rec);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].winner_slot, 0);
        assert_eq!(got[1].winner_slot, 1);
        assert_eq!(
            got[1].chars,
            ["Orcane".to_string(), "Zetterburn".to_string()]
        );
    }

    #[test]
    fn a_set_with_no_recorded_games_still_opens_with_one() {
        // The case the editor exists for: the station recorded nothing usable
        // and the operator is entering the set by hand.
        let got = games_from_record(&json!({"set": {"games": []}}));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].chars, [String::new(), String::new()]);
        assert_eq!(games_from_record(&json!({})).len(), 1, "no set at all");
    }

    #[test]
    fn the_editor_shows_the_score_its_games_add_up_to() {
        // The score is derived, never typed — it cannot disagree with the
        // games, and the games are what start.gg is told.
        let gs = games(5);
        let el = result_editor(&gs, ["LOOM".into(), "SLADE".into()], 3, "s1", false, false);
        let mut ui = iced_test::simulator(el);
        assert!(ui.find("Correct the result").is_ok());
        assert!(
            ui.find("LOOM 3–2 SLADE").is_ok(),
            "three games won by slot 0, two by slot 1"
        );
        assert!(ui.find("G5").is_ok(), "one line per game");
        assert!(ui.find("Save result").is_ok());
    }

    #[test]
    fn a_drawn_correction_cannot_be_saved() {
        // The hub would store it with no winner and keep it off the bracket;
        // saying so here is better than letting it look accepted.
        let gs = games(2);
        let el = result_editor(&gs, ["LOOM".into(), "SLADE".into()], 3, "s1", false, false);
        let mut ui = iced_test::simulator(el);
        assert!(ui.find("LOOM 1–1 SLADE").is_ok());
        assert!(ui.find("a set needs a winner").is_ok());
    }

    #[test]
    fn character_options_come_from_the_one_canonical_table() {
        let opts = character_options();
        assert_eq!(opts[0], "", "blank first: no character recorded is real");
        assert!(opts.iter().any(|c| c == "Zetterburn"));
        assert!(opts.iter().any(|c| c == "La Reina"), "multi-word names");
        assert!(opts.iter().any(|c| c == "Gouie"), "and newer ones");
        assert!(
            !opts.iter().any(|c| c == "Random"),
            "Random is a pick screen state, not something to report"
        );
    }
}
