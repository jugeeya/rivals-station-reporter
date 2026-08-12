//! The Bracket screen — the whole tree, so a TO never has to leave the app
//! for start.gg's own page mid-event.
//!
//! Reading is unauthenticated (see `bracket::fetch`), so this renders on a
//! station install too. Acting is not: assigning a station, calling a match
//! and reporting a result are bracket writes, and they go through the
//! operator's token exactly like the Current Sets panel's do. A station
//! install therefore sees the same tree with the action bar explaining why
//! it's read-only there.
//!
//! Clicking a set selects it and opens the action bar underneath; the tree
//! itself never carries a button that writes to start.gg, so a mis-click
//! while panning around a bracket can't advance anyone.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, Space};
use iced::{Alignment, Element, Length, Task};
use serde_json::json;

use super::{blocking, chars, format, App, Message, Screen};
use crate::bracket::fetch::{self, Bracket, BracketSet};
use crate::bracket::layout::{self, Column, GroupKey};
use crate::engine::commands;
use crate::theme;

/// Every card is this wide, so the columns line up into readable rounds no
/// matter how long the tags in any one of them are.
const CARD_W: f32 = 236.0;
/// Fixed, so the connector canvas can work out where every card's edges are
/// without asking the renderer.
const CARD_H: f32 = 64.0;
/// Character stock icon, sized to the tag beside it.
const ICON: f32 = 18.0;
const COLUMN_GAP: f32 = 28.0;
const CARD_GAP: f32 = 12.0;
/// One layout row: a card plus the gap under it.
const PITCH: f32 = CARD_H + CARD_GAP;
/// The picker entries that leave a set's destination as start.gg has it.
const NO_STATION: &str = "leave station as is";
const NO_STREAM: &str = "leave stream as is";

#[derive(Debug, Clone)]
pub enum Msg {
    Refresh,
    Loaded(u64, Box<Result<Bracket, String>>),
    PickGroup(GroupKey),
    Select(String),
    Deselect,
    PickStation(String),
    PickStream(String),
    StartMatch,
    /// Finalize the selected set with this entrant id as the winner.
    Report(String),
    /// Reveal (or hide) the winner buttons on a set the bracket already has a
    /// result for. Deliberately a second click: re-reporting resets the set on
    /// start.gg, so it must never be one stray tap away.
    ToggleChangeResult,
    /// Replace an already-reported result with this entrant as the winner.
    Rereport(String),
    ActionDone(Box<Result<String, String>>),
    Close,
}

#[derive(Default)]
pub struct State {
    pub slug: String,
    pub bracket: Option<Bracket>,
    /// Which phase group is on screen. `None` until the first load picks one.
    pub group: Option<GroupKey>,
    pub loading: bool,
    pub load_err: String,
    /// Selected set id, the one the action bar acts on.
    pub selected: Option<String>,
    pub picked_station: Option<String>,
    pub picked_stream: Option<String>,
    /// The selected set is reported and the operator asked to change it.
    pub changing_result: bool,
    pub busy: bool,
    pub action_msg: String,
    pub action_err: bool,
    /// Guards against an older in-flight load overwriting a newer one.
    gen: u64,
}

impl State {
    fn selected_set(&self) -> Option<&BracketSet> {
        let id = self.selected.as_deref()?;
        self.bracket.as_ref()?.sets.iter().find(|s| s.id == id)
    }

    /// The station number the action bar would send, or `None` for "leave it".
    fn station_choice(&self) -> Option<i64> {
        self.picked_station
            .as_deref()
            .and_then(|v| v.strip_prefix("Station "))
            .and_then(|v| v.parse().ok())
    }

    /// Same for the stream setup. Both halves are independent: a set can be
    /// on a station AND on a stream at once (see `Destination::from_parts`).
    fn stream_choice(&self) -> Option<String> {
        self.picked_stream
            .as_deref()
            .and_then(|v| v.strip_prefix("Stream: "))
            .map(str::to_string)
    }

    /// Seed both pickers from where start.gg already has this set, so acting
    /// without touching them is a no-op reassignment rather than a move.
    fn seed_pickers(&mut self) {
        let set = self.selected_set();
        let station = set.and_then(|s| s.station);
        let stream = set.and_then(|s| s.stream.clone()).filter(|s| !s.is_empty());
        self.picked_station = station.map(|n| format!("Station {n}"));
        self.picked_stream = stream.map(|s| format!("Stream: {s}"));
    }
}

// ---- loading -----------------------------------------------------------------

pub fn refresh(app: &mut App) -> Task<Message> {
    let slug = app.bracket.slug.clone();
    if slug.trim().is_empty() {
        app.bracket.load_err = "No event configured — set the event link in Settings.".into();
        return Task::none();
    }
    app.bracket.gen += 1;
    let gen = app.bracket.gen;
    app.bracket.loading = true;
    app.bracket.load_err.clear();
    Task::perform(fetch::fetch(slug), move |r| {
        Message::Bracket(Msg::Loaded(gen, Box::new(r)))
    })
}

/// Called when the screen opens: adopt the reporter's configured event and
/// pull the tree. Always re-fetches — sets finish while the screen is closed,
/// and a stale bracket is worse than a spinner.
pub fn opened(app: &mut App) -> Task<Message> {
    if app.bracket.slug != app.st.config.slug {
        app.bracket.slug = app.st.config.slug.clone();
        // Different event entirely: nothing selected there is meaningful here.
        app.bracket.bracket = None;
        app.bracket.group = None;
        app.bracket.selected = None;
    }
    refresh(app)
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    match msg {
        Msg::Close => {
            app.screen = Screen::Reporter;
            Task::none()
        }
        Msg::Refresh => refresh(app),
        Msg::Loaded(gen, result) => {
            // A refresh fired after this one has already landed; dropping the
            // older answer keeps the newer tree on screen.
            if gen != app.bracket.gen {
                return Task::none();
            }
            let st = &mut app.bracket;
            st.loading = false;
            match *result {
                Ok(b) => {
                    let keys = layout::groups_of(&b);
                    // Keep the operator on the group they were looking at,
                    // unless it vanished (phase not started, event swapped).
                    let keep = st
                        .group
                        .as_ref()
                        .filter(|g| keys.iter().any(|k| k.id == g.id))
                        .cloned();
                    st.group = keep.or_else(|| keys.first().cloned());
                    // A selected set that's no longer in the bracket (or has
                    // since been reported) must not keep the action bar open
                    // pointing at something that isn't there.
                    if let Some(id) = st.selected.clone() {
                        if !b.sets.iter().any(|s| s.id == id) {
                            st.selected = None;
                        }
                    }
                    // Opening onto a bracket with nothing selected, land on
                    // whatever the TO is most likely there to deal with —
                    // the set that's been playing longest, else the next one
                    // callable — rather than making them hunt for it.
                    if st.selected.is_none() {
                        if let Some(g) = &st.group {
                            if let Some(s) = layout::set_needing_attention(&b, &g.id) {
                                st.selected = Some(s.id.clone());
                            }
                        }
                    }
                    st.bracket = Some(b);
                    st.load_err.clear();
                    // After the tree lands, so the pickers read the freshly
                    // fetched assignment rather than the one they were on.
                    st.seed_pickers();
                }
                Err(e) => {
                    st.load_err = e;
                    st.bracket = None;
                }
            }
            Task::none()
        }
        Msg::PickGroup(key) => {
            app.bracket.group = Some(key);
            app.bracket.selected = None;
            app.bracket.changing_result = false;
            Task::none()
        }
        Msg::Select(id) => {
            let st = &mut app.bracket;
            // Clicking the selected set again closes the action bar.
            if st.selected.as_deref() == Some(id.as_str()) {
                st.selected = None;
            } else {
                st.selected = Some(id);
                st.seed_pickers();
            }
            // A different set's result is not the one the operator just asked
            // to change.
            st.changing_result = false;
            st.action_msg.clear();
            st.action_err = false;
            Task::none()
        }
        Msg::Deselect => {
            app.bracket.selected = None;
            app.bracket.changing_result = false;
            Task::none()
        }
        Msg::PickStation(v) => {
            app.bracket.picked_station = (v != NO_STATION).then_some(v);
            Task::none()
        }
        Msg::PickStream(v) => {
            app.bracket.picked_stream = (v != NO_STREAM).then_some(v);
            Task::none()
        }
        Msg::ToggleChangeResult => {
            app.bracket.changing_result = !app.bracket.changing_result;
            app.bracket.action_msg.clear();
            app.bracket.action_err = false;
            Task::none()
        }
        Msg::StartMatch => {
            let Some(set) = app.bracket.selected_set() else {
                return Task::none();
            };
            let id = set.id.clone();
            let station = app.bracket.station_choice();
            let stream = app.bracket.stream_choice();
            app.bracket.busy = true;
            app.bracket.action_msg.clear();
            let engine = app.engine.clone();
            Task::perform(
                blocking(move || {
                    let told = match (station, stream.clone()) {
                        (Some(n), Some(s)) => format!("Called to station {n}, on {s}."),
                        (Some(n), None) => format!("Called to station {n}."),
                        (None, Some(s)) => format!("Put on {s}."),
                        (None, None) => "Match started.".to_string(),
                    };
                    commands::start_match(&engine, &id, station, stream).map(|_| told)
                }),
                |r| Message::Bracket(Msg::ActionDone(Box::new(r))),
            )
        }
        Msg::Report(winner_id) => {
            let Some(set) = app.bracket.selected_set() else {
                return Task::none();
            };
            let id = set.id.clone();
            let winner_name = set
                .slots
                .iter()
                .find(|s| s.entrant_id.as_deref() == Some(winner_id.as_str()))
                .and_then(|s| s.name.clone())
                .unwrap_or_else(|| "winner".into());
            app.bracket.busy = true;
            app.bracket.action_msg.clear();
            let engine = app.engine.clone();
            Task::perform(
                blocking(move || {
                    commands::report_bracket_set(&engine, &id, &json!(winner_id))
                        .map(|_| format!("Reported — {winner_name} advances."))
                }),
                |r| Message::Bracket(Msg::ActionDone(Box::new(r))),
            )
        }
        Msg::Rereport(winner_id) => {
            let Some(set) = app.bracket.selected_set() else {
                return Task::none();
            };
            let id = set.id.clone();
            let winner_name = set
                .slots
                .iter()
                .find(|s| s.entrant_id.as_deref() == Some(winner_id.as_str()))
                .and_then(|s| s.name.clone())
                .unwrap_or_else(|| "winner".into());
            app.bracket.busy = true;
            app.bracket.action_msg.clear();
            let engine = app.engine.clone();
            Task::perform(
                blocking(move || {
                    commands::rereport_bracket_set(&engine, &id, &json!(winner_id))
                        .map(|_| format!("Result changed — {winner_name} advances."))
                }),
                |r| Message::Bracket(Msg::ActionDone(Box::new(r))),
            )
        }
        Msg::ActionDone(result) => {
            let st = &mut app.bracket;
            st.busy = false;
            match *result {
                Ok(msg) => {
                    st.action_msg = msg;
                    st.action_err = false;
                    // Whatever was being changed is changed; don't leave the
                    // winner buttons open over a fresh result.
                    st.changing_result = false;
                    // The tree just changed underneath us — an advanced set
                    // seeds the next round, so re-read rather than patch.
                    refresh(app)
                }
                Err(e) => {
                    st.action_msg = e;
                    st.action_err = true;
                    Task::none()
                }
            }
        }
    }
}

// ---- view ---------------------------------------------------------------------

/// Everything the view needs that doesn't live in `State`. A plain value
/// rather than a borrow of `App`, so the whole tree renders in a test without
/// an engine behind it — the same reason the other screens take `&State`.
pub struct Ctx {
    pub now_s: i64,
    /// Why this install can't act on the bracket at all, if it can't. Set-level
    /// reasons (waiting on a round) are decided per set.
    pub blocked: Option<String>,
}

impl Ctx {
    fn of(app: &App) -> Self {
        let cfg = &app.st.config;
        Self {
            now_s: app.now_s,
            blocked: if !cfg.configured || cfg.mode == "station" {
                Some("Bracket actions run on the operator PC.".into())
            } else if cfg.startgg_token.is_empty() {
                Some("No start.gg API token configured — add one in Settings.".into())
            } else {
                None
            },
        }
    }
}

pub fn view(app: &App) -> Element<'_, Message> {
    screen(&app.bracket, &Ctx::of(app)).map(Message::Bracket)
}

fn screen<'a>(st: &'a State, ctx: &Ctx) -> Element<'a, Msg> {
    let mut title = row![
        text("Bracket").font(theme::FONT_DISPLAY).size(20),
        text(
            st.bracket
                .as_ref()
                .map(|b| {
                    if b.tournament_name.is_empty() {
                        b.event_name.clone()
                    } else {
                        format!("{} · {}", b.tournament_name, b.event_name)
                    }
                })
                .unwrap_or_default()
        )
        .size(13)
        .color(theme::TEXT_MUTED),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Only worth a picker when there's more than one group to pick.
    let keys = st
        .bracket
        .as_ref()
        .map(layout::groups_of)
        .unwrap_or_default();
    if keys.len() > 1 {
        title = title.push(
            pick_list(keys, st.group.clone(), Msg::PickGroup)
                .text_size(13)
                .style(theme::pick_list_style)
                .menu_style(theme::pick_list_menu),
        );
    }

    let header = row![
        title,
        Space::new().width(Length::Fill),
        button(
            text(if st.loading {
                "Refreshing…"
            } else {
                "Refresh"
            })
            .size(13)
        )
        .style(theme::button_surface)
        .on_press_maybe((!st.loading).then_some(Msg::Refresh)),
        super::view_toggle(true, Msg::Close, Msg::Close),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let body: Element<'_, Msg> = match (&st.bracket, st.group.as_ref()) {
        _ if !st.load_err.is_empty() => notice(&st.load_err, true),
        (Some(b), Some(group)) => tree(b, group, st.selected.as_deref()),
        (Some(_), None) => notice("This event has no bracket yet.", false),
        (None, _) if st.loading => notice("Reading the bracket…", false),
        (None, _) => notice("Nothing loaded yet.", false),
    };

    // A phase that has not been started on start.gg reports its sets with
    // placeholder ids. Starting any one of them materialises the whole phase
    // (see `START_MATCH_MUTATION`), so this is a note about what the next
    // click will do rather than a dead end — but it is worth saying, because
    // nothing will report itself until someone does it.
    let unstarted = st
        .bracket
        .as_ref()
        .zip(st.group.as_ref())
        .map(|(b, g)| {
            let mine: Vec<_> = b.sets.iter().filter(|x| x.phase_group_id == g.id).collect();
            !mine.is_empty() && mine.iter().all(|x| x.preview)
        })
        .unwrap_or(false);

    let mut content = column![header].spacing(14);
    if unstarted {
        content = content.push(
            container(
                text(
                    "This bracket hasn't been started on start.gg yet. Starting any match here starts it — the sets become real and everything else works normally.",
                )
                .size(12)
                .color(theme::TEXT_WARNING),
            )
            .style(theme::panel_warning)
            .padding(10)
            .width(Length::Fill),
        );
    }
    content = content.push(body);
    if let Some(bar) = action_bar(st, ctx) {
        content = content.push(bar);
    }

    container(content)
        .style(theme::card_rich)
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn notice(msg: &str, bad: bool) -> Element<'_, Msg> {
    container(text(msg.to_string()).size(13).color(if bad {
        theme::TEXT_FAILURE
    } else {
        theme::TEXT_MUTED
    }))
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Winners above, losers below, each a row of round columns. One scrollable
/// over both so the two sides pan together and stay lined up by round.
fn tree<'a>(bracket: &'a Bracket, group: &GroupKey, selected: Option<&str>) -> Element<'a, Msg> {
    let laid = layout::lay_out(bracket, &group.id);
    if laid.is_empty() {
        return notice("This group has no sets on start.gg yet.", false);
    }

    // Only a double-elimination group has two sides worth naming; anything
    // else (single elim, a pool's own bracket) puts everything on the
    // positive-round side, where "Winners" would be a lie.
    let two_sided = bracket
        .sets
        .iter()
        .any(|s| s.phase_group_id == group.id && s.bracket_type == "DOUBLE_ELIMINATION");

    let mut inner = column![].spacing(22);
    for (label, side) in [
        (if two_sided { "Winners" } else { "Bracket" }, &laid.winners),
        ("Losers", &laid.losers),
    ] {
        if side.columns.is_empty() {
            continue;
        }
        // Titles and cards are two separate rows sharing a width and a gap,
        // so every round name lines up along one baseline. Putting each title
        // inside its own column instead staggers them.
        let mut titles = row![].spacing(COLUMN_GAP);
        let mut cards = row![].spacing(COLUMN_GAP).align_y(Alignment::Start);
        for col in &side.columns {
            titles = titles.push(round_title(col));
            cards = cards.push(round_column(col, selected));
        }

        // The connectors go UNDER the cards, in a canvas sized to exactly the
        // same box the card row occupies. Both layers start at the stack's
        // origin and every card's position is arithmetic we did ourselves
        // (fixed card size, fixed gaps, rows from the layout), so the elbows
        // land on the card edges without the renderer having to tell us where
        // anything ended up.
        let width = side.columns.len() as f32 * CARD_W
            + (side.columns.len().saturating_sub(1)) as f32 * COLUMN_GAP;
        let height = side.rows() * PITCH - CARD_GAP;
        let wired = iced::widget::stack![
            iced::widget::canvas(Connectors {
                links: side.links.clone(),
            })
            .width(Length::Fixed(width))
            .height(Length::Fixed(height.max(1.0))),
            cards,
        ];

        inner = inner.push(
            column![
                text(theme::tracked(label))
                    .font(theme::FONT_BODY_SEMIBOLD)
                    .size(10)
                    .color(theme::TEXT_MUTED),
                titles,
                wired,
            ]
            .spacing(8),
        );
    }

    scrollable(container(inner).padding([4, 2]))
        .direction(Direction::Both {
            vertical: Scrollbar::new(),
            horizontal: Scrollbar::new(),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn round_title<'a>(col: &Column<'a>) -> Element<'a, Msg> {
    container(
        text(col.title)
            .size(11)
            .font(theme::FONT_BODY_MEDIUM)
            .color(theme::TEXT_MUTED)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .width(Length::Fixed(CARD_W))
    .clip(true)
    .into()
}

/// One column, with each card pushed down to the row the layout gave it.
/// Spacers rather than `spacing()`: the rows are fractional (a set fed by two
/// others sits halfway between them), and they're what the connector canvas
/// draws against, so the two have to agree exactly.
fn round_column<'a>(col: &Column<'a>, selected: Option<&str>) -> Element<'a, Msg> {
    let mut out = column![];
    let mut filled = 0.0f32;
    for placed in &col.sets {
        let top = placed.row * PITCH;
        if top > filled {
            out = out.push(Space::new().height(Length::Fixed(top - filled)));
        }
        out = out.push(set_card(
            placed.set,
            selected == Some(placed.set.id.as_str()),
        ));
        filled = top + CARD_H;
    }
    container(out).width(Length::Fixed(CARD_W)).into()
}

/// The elbows between a set and the sets that feed it. Drawn from the same
/// (column, row) coordinates the cards are placed at.
struct Connectors {
    links: Vec<layout::Link>,
}

impl<Message> iced::widget::canvas::Program<Message> for Connectors {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path, Stroke};

        let mut frame = Frame::new(renderer, bounds.size());
        let stroke = Stroke::default()
            .with_color(theme::LINE_CONNECTOR)
            .with_width(1.5);

        for link in &self.links {
            let col_x = |c: usize| c as f32 * (CARD_W + COLUMN_GAP);
            let mid_y = |r: f32| r * PITCH + CARD_H / 2.0;
            // Out of the feeder's right edge, across half the gutter, down or
            // up to the successor's height, then into its left edge.
            let x1 = col_x(link.from_col) + CARD_W;
            let y1 = mid_y(link.from_row);
            let x2 = col_x(link.to_col);
            let y2 = mid_y(link.to_row);
            let elbow = (x1 + x2) / 2.0;
            let path = Path::new(|b| {
                b.move_to(iced::Point::new(x1, y1));
                b.line_to(iced::Point::new(elbow, y1));
                b.line_to(iced::Point::new(elbow, y2));
                b.line_to(iced::Point::new(x2, y2));
            });
            frame.stroke(&path, stroke.clone());
        }

        vec![frame.into_geometry()]
    }
}

fn set_card(set: &BracketSet, is_selected: bool) -> Element<'_, Msg> {
    let seat = |i: usize| -> Element<'_, Msg> {
        let slot = &set.slots[i];
        let won = set.winner_slot() == Some(i);
        let name = slot.name.clone().unwrap_or_else(|| "—".into());
        let color = if slot.name.is_none() {
            theme::TEXT_MUTED
        } else if won {
            theme::TEXT_SUCCESS
        } else if set.is_complete() {
            theme::TEXT_MUTED
        } else {
            theme::TEXT_PRIMARY
        };
        // Stock icon then tag. The icon is a fixed-width column of its own, so
        // tags start at the same x whether or not a set has game data yet
        // (unplayed sets have no character at all).
        let portrait: Element<'_, Msg> = match &slot.character {
            Some(c) => chars::char_icon(Some(c), ICON),
            None => Space::new().width(Length::Fixed(ICON)).into(),
        };

        row![
            portrait,
            container(
                text(name)
                    .size(13)
                    .font(if won {
                        theme::FONT_BODY_BOLD
                    } else {
                        theme::FONT_BODY
                    })
                    .color(color)
                    .wrapping(iced::widget::text::Wrapping::None)
            )
            .width(Length::Fill)
            .clip(true),
            text(slot.score_text())
                .size(13)
                .font(theme::FONT_MONO)
                .color(if slot.is_dq() {
                    theme::TEXT_FAILURE
                } else {
                    color
                }),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    };

    // Top line: where this set is and whether it needs attention. No set
    // label — across a full bracket it is noise, and it reads as an id that
    // means something it doesn't.
    //
    // The station shows only on sets that HAVEN'T finished: on a played set
    // it is history nobody is looking for, but on a live or upcoming one it
    // is the thing a TO walks to.
    let ready = set.is_startable();
    let mut top = row![Space::new().width(Length::Fill)]
        .spacing(6)
        .align_y(Alignment::Center);
    if set.is_ongoing() {
        top = top.push(text("● live").size(10).color(theme::ACCENT_HOVER));
    } else if set.is_called() {
        top = top.push(text("called").size(10).color(theme::TEXT_WARNING));
    } else if ready {
        top = top.push(
            text("ready")
                .size(10)
                .font(theme::FONT_BODY_SEMIBOLD)
                .color(theme::TEXT_WARNING),
        );
    }
    if !set.is_complete() {
        if let Some(n) = set.station {
            top = top.push(text(format!("St {n}")).size(10).color(theme::TEXT_MUTED));
        }
    }
    if let Some(s) = &set.stream {
        top = top.push(text(s.clone()).size(10).color(theme::TEXT_MUTED));
    }

    let style = if is_selected {
        theme::bracket_set_selected
    } else if set.is_ongoing() {
        theme::bracket_set_live
    } else if set.is_complete() {
        theme::bracket_set_done
    } else if ready {
        theme::bracket_set_ready
    } else {
        theme::bracket_set
    };

    // Fixed height, not content height: the connector canvas computes card
    // edges arithmetically, so a card that grew by a line would put every
    // elbow below it out by that much.
    button(column![top, seat(0), seat(1)].spacing(3))
        .style(style)
        .height(Length::Fixed(CARD_H))
        .padding([7, 10])
        .width(Length::Fixed(CARD_W))
        .on_press(Msg::Select(set.id.clone()))
        .into()
}

// ---- screenshot seeding --------------------------------------------------------

/// Fixture bracket for a capture (the `bracket` key of `RSR_SEED_STATE`).
/// The real screen reads over the network, which a CI runner rendering the
/// README shots can't do — and shouldn't, since a shot of a live bracket
/// would change every time someone played a set.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Seed {
    #[serde(default)]
    pub event_name: String,
    #[serde(default)]
    pub tournament_name: String,
    /// Set id to open the action bar on, instead of letting
    /// `set_needing_attention` choose.
    #[serde(default)]
    pub selected: Option<String>,
    /// Open the winner buttons on an already-reported selected set, as if the
    /// operator had clicked "Change result".
    #[serde(default)]
    pub changing_result: bool,
    /// The tournament's stations and streams, for the action bar's pickers.
    #[serde(default)]
    pub stations: Vec<i64>,
    #[serde(default)]
    pub streams: Vec<String>,
    #[serde(default)]
    pub sets: Vec<SeedSet>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SeedSet {
    pub id: String,
    /// Placeholder set from a bracket start.gg hasn't started yet.
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    pub state: i64,
    #[serde(default)]
    pub round: i64,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub full_round_text: String,
    #[serde(default)]
    pub winner_id: Option<String>,
    #[serde(default)]
    pub total_games: Option<i64>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub station: Option<i64>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub slots: Vec<SeedSlot>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SeedSlot {
    #[serde(default)]
    pub entrant_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub score: Option<i64>,
    #[serde(default)]
    pub character: Option<String>,
    /// Feeding set id, so a fixture draws the same connectors a real bracket
    /// does rather than a disconnected grid.
    #[serde(default)]
    pub prereq_set_id: Option<String>,
}

fn seed_slot(s: Option<&SeedSlot>) -> fetch::Slot {
    match s {
        Some(s) => fetch::Slot {
            entrant_id: s.entrant_id.clone(),
            name: s.name.clone(),
            score: s.score,
            character: s.character.clone(),
            prereq_set_id: s.prereq_set_id.clone(),
        },
        None => fetch::Slot::default(),
    }
}

pub fn apply_seed(app: &mut App, seed: Seed) {
    let sets: Vec<BracketSet> = seed
        .sets
        .into_iter()
        .map(|s| BracketSet {
            id: s.id,
            preview: s.preview,
            state: s.state,
            round: s.round,
            identifier: s.identifier,
            full_round_text: s.full_round_text,
            winner_id: s.winner_id,
            total_games: s.total_games,
            started_at: s.started_at,
            completed_at: s.completed_at,
            station: s.station,
            stream: s.stream,
            // One group is all a fixture ever needs; the picker hides itself.
            phase_group_id: "seed".into(),
            phase_group_label: "1".into(),
            phase_name: "Bracket".into(),
            phase_order: 1,
            bracket_type: "DOUBLE_ELIMINATION".into(),
            slots: [seed_slot(s.slots.first()), seed_slot(s.slots.get(1))],
        })
        .collect();

    let bracket = Bracket {
        event_name: seed.event_name,
        tournament_name: seed.tournament_name,
        stations: seed.stations,
        streams: seed.streams,
        sets,
    };
    let st = &mut app.bracket;
    st.group = layout::groups_of(&bracket).into_iter().next();
    st.selected = seed.selected.or_else(|| {
        st.group
            .as_ref()
            .and_then(|g| layout::set_needing_attention(&bracket, &g.id))
            .map(|s| s.id.clone())
    });
    st.bracket = Some(bracket);
    st.loading = false;
    st.load_err.clear();
    st.changing_result = seed.changing_result;
    st.seed_pickers();
}

/// The one place in this screen that writes to start.gg. Only appears for a
/// selected set, and only offers what that set can actually accept.
fn action_bar<'a>(st: &'a State, ctx: &Ctx) -> Option<Element<'a, Msg>> {
    let set = st.selected_set()?;

    let title = format!(
        "{} · {}",
        if set.full_round_text.is_empty() {
            set.identifier.clone()
        } else {
            set.full_round_text.clone()
        },
        set.slots
            .iter()
            .map(|s| s.name.clone().unwrap_or_else(|| "—".into()))
            .collect::<Vec<_>>()
            .join(" vs ")
    );

    // Everything about the set that isn't its name: where it is, how long it
    // is, and when it ran. Exactly what a TO asks before calling or reporting
    // one — and the only place the station appears, since across the whole
    // tree it was noise.
    let mut meta: Vec<String> = Vec::new();
    if let Some(n) = set.station {
        meta.push(format!("station {n}"));
    }
    if let Some(s) = &set.stream {
        meta.push(format!("stream {s}"));
    }
    if let Some(b) = format::best_of(set.total_games) {
        meta.push(b);
    }
    match (set.started_at, set.completed_at) {
        (Some(start), Some(end)) => meta.push(format!(
            "{}–{} ({})",
            format::clock(start),
            format::clock(end),
            format::elapsed_since(start, end)
        )),
        (Some(start), None) => meta.push(format!(
            "started {} · {}",
            format::clock(start),
            format::elapsed_since(start, ctx.now_s)
        )),
        _ => {}
    }

    let mut head = row![
        text(title)
            .size(13)
            .font(theme::FONT_BODY_MEDIUM)
            .color(theme::TEXT_PRIMARY),
        text(meta.join(" · ")).size(12).color(theme::TEXT_MUTED),
        Space::new().width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if !st.action_msg.is_empty() {
        head = head.push(
            text(st.action_msg.clone())
                .size(12)
                .color(if st.action_err {
                    theme::TEXT_FAILURE
                } else {
                    theme::TEXT_SUCCESS
                }),
        );
    }
    head = head.push(
        button(text("✕").size(12))
            .style(theme::button_linkish)
            .on_press(Msg::Deselect),
    );

    // Whether this install can write to the bracket at all is a property of
    // the install, said once, plainly. What a PARTICULAR set can accept is
    // then decided per set below.
    // Always `Some` here — `selected_set` above resolved through it.
    let bracket = st.bracket.as_ref()?;
    let actions: Element<'_, Msg> = if let Some(why) = &ctx.blocked {
        text(why.clone()).size(12).color(theme::TEXT_MUTED).into()
    } else if set.is_complete() {
        reported_actions(st, set)
    } else if !set.is_ready() {
        text("Waiting on an earlier round.")
            .size(12)
            .color(theme::TEXT_MUTED)
            .into()
    } else {
        callable_actions(st, set, bracket)
    };

    Some(
        container(column![head, actions].spacing(10))
            .style(theme::panel)
            .padding(14)
            .width(Length::Fill)
            .into(),
    )
}

/// A set the bracket already has a result for.
///
/// This is the one place in the app that can fix such a set: the operator
/// console can only correct sets one of ITS stations recorded, and by top 8
/// the results worth fixing are usually on sets no station saw — a stream
/// setup, a phone report, or an auto-report that read a tag wrong. So the
/// bracket offers it, behind a deliberate second click.
fn reported_actions<'a>(st: &'a State, set: &'a BracketSet) -> Element<'a, Msg> {
    let winner = set
        .winner_slot()
        .and_then(|i| set.slots[i].name.clone())
        .unwrap_or_else(|| "someone".into());

    if !st.changing_result {
        return row![
            text(format!("Reported — {winner} advanced."))
                .size(12)
                .color(theme::TEXT_MUTED),
            button(text("Change result").size(12))
                .style(theme::button_surface)
                .on_press(Msg::ToggleChangeResult),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into();
    }

    let mut picks = row![text(theme::tracked("New winner"))
        .size(10)
        .font(theme::FONT_BODY_SEMIBOLD)
        .color(theme::TEXT_MUTED)]
    .spacing(8)
    .align_y(Alignment::Center);
    for (i, slot) in set.slots.iter().enumerate() {
        let (Some(id), Some(name)) = (slot.entrant_id.clone(), slot.name.clone()) else {
            continue;
        };
        // Re-picking whoever already won only rewrites the same result, so the
        // OTHER seat is the action actually on offer here — and reads that way.
        let already_won = set.winner_slot() == Some(i);
        picks = picks.push(
            button(text(name).size(13))
                .style(if already_won {
                    theme::button_surface
                } else {
                    theme::button_primary_rich
                })
                .on_press_maybe((!st.busy).then_some(Msg::Rereport(id))),
        );
    }
    picks = picks.push(
        button(text("Cancel").size(12))
            .style(theme::button_linkish)
            .on_press(Msg::ToggleChangeResult),
    );

    column![
        picks,
        // The one consequence that isn't undone for you: `resetSet` is called
        // without `resetDependentSets`, on purpose, so a corrected score can't
        // silently unseed rounds that have already been played.
        text(
            "Resets the set on start.gg, then reports the new winner. Later rounds already \
             played out of this set keep their results — fix those here too if they changed."
        )
        .size(11)
        .color(theme::TEXT_WARNING),
    ]
    .spacing(6)
    .into()
}

/// A set that can still be called, moved, or reported for the first time.
fn callable_actions<'a>(
    st: &'a State,
    set: &'a BracketSet,
    bracket: &'a Bracket,
) -> Element<'a, Msg> {
    let mut station_opts: Vec<String> = vec![NO_STATION.to_string()];
    station_opts.extend(bracket.stations.iter().map(|n| format!("Station {n}")));
    let mut r = row![pick_list(
        station_opts,
        Some(
            st.picked_station
                .clone()
                .unwrap_or_else(|| NO_STATION.to_string())
        ),
        Msg::PickStation,
    )
    .text_size(13)
    .style(theme::pick_list_style)
    .menu_style(theme::pick_list_menu)]
    .spacing(8)
    .align_y(Alignment::Center);

    // Stream picker only when the tournament actually has stream setups —
    // most locals don't, and an empty dropdown is just a thing to wonder about.
    // Separate from the station picker because a set can be on both at once.
    if !bracket.streams.is_empty() {
        let mut stream_opts: Vec<String> = vec![NO_STREAM.to_string()];
        stream_opts.extend(bracket.streams.iter().map(|s| format!("Stream: {s}")));
        r = r.push(
            pick_list(
                stream_opts,
                Some(
                    st.picked_stream
                        .clone()
                        .unwrap_or_else(|| NO_STREAM.to_string()),
                ),
                Msg::PickStream,
            )
            .text_size(13)
            .style(theme::pick_list_style)
            .menu_style(theme::pick_list_menu),
        );
    }

    r = r.push(
        button(
            text(if set.is_ongoing() {
                "Re-call"
            } else {
                "Start match"
            })
            .size(13),
        )
        .style(theme::button_surface)
        .on_press_maybe((!st.busy).then_some(Msg::StartMatch)),
    );
    r = r.push(Space::new().width(Length::Fixed(12.0)));

    if set.preview {
        // The set has no id on start.gg to report against yet. Starting it
        // (above) is what gives it one — and starts the whole bracket — so say
        // that instead of offering a button that could only fail.
        return r
            .push(
                text("start it first — the bracket isn't live on start.gg yet")
                    .size(12)
                    .color(theme::TEXT_MUTED),
            )
            .into();
    }

    r = r.push(
        text(theme::tracked("Winner"))
            .size(10)
            .font(theme::FONT_BODY_SEMIBOLD)
            .color(theme::TEXT_MUTED),
    );
    for slot in &set.slots {
        let (Some(id), Some(name)) = (slot.entrant_id.clone(), slot.name.clone()) else {
            continue;
        };
        r = r.push(
            button(text(name).size(13))
                .style(theme::button_primary_rich)
                .on_press_maybe((!st.busy).then_some(Msg::Report(id))),
        );
    }
    r.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> State {
        // The same fixture the README shot uses, trimmed to what a test needs:
        // a finished set, a live one, and one still waiting on a seat.
        let seed = Seed {
            event_name: "Rivals 2 Singles".into(),
            tournament_name: "The Hangout #47".into(),
            selected: Some("w-g".into()),
            changing_result: false,
            stations: vec![1, 2, 3],
            streams: vec!["socalrivals".into()],
            sets: vec![
                SeedSet {
                    id: "w-c".into(),
                    state: fetch::STATE_COMPLETED,
                    round: 2,
                    identifier: "C".into(),
                    full_round_text: "Winners Quarter-Final".into(),
                    winner_id: Some("E1".into()),
                    station: Some(1),
                    slots: vec![
                        SeedSlot {
                            entrant_id: Some("E1".into()),
                            name: Some("jugeeya".into()),
                            score: Some(3),
                            ..SeedSlot::default()
                        },
                        SeedSlot {
                            entrant_id: Some("E9".into()),
                            name: Some("Marsh".into()),
                            score: Some(2),
                            ..SeedSlot::default()
                        },
                    ],
                    ..SeedSet::default()
                },
                SeedSet {
                    id: "w-g".into(),
                    state: fetch::STATE_ONGOING,
                    round: 3,
                    identifier: "G".into(),
                    full_round_text: "Winners Semi-Final".into(),
                    station: Some(2),
                    total_games: Some(5),
                    slots: vec![
                        SeedSlot {
                            entrant_id: Some("E3".into()),
                            name: Some("BRUJITA".into()),
                            score: Some(2),
                            ..SeedSlot::default()
                        },
                        SeedSlot {
                            entrant_id: Some("E4".into()),
                            name: Some("NAVI".into()),
                            score: Some(1),
                            ..SeedSlot::default()
                        },
                    ],
                    ..SeedSet::default()
                },
                SeedSet {
                    id: "w-i".into(),
                    round: 4,
                    identifier: "I".into(),
                    full_round_text: "Winners Final".into(),
                    slots: vec![SeedSlot::default(), SeedSlot::default()],
                    ..SeedSet::default()
                },
            ],
        };
        // apply_seed wants an App; build the same State it would produce.
        let sets: Vec<BracketSet> = seed
            .sets
            .into_iter()
            .map(|s| BracketSet {
                id: s.id,
                preview: false,
                state: s.state,
                round: s.round,
                identifier: s.identifier,
                full_round_text: s.full_round_text,
                winner_id: s.winner_id,
                total_games: s.total_games,
                started_at: s.started_at,
                completed_at: s.completed_at,
                station: s.station,
                stream: s.stream,
                phase_group_id: "seed".into(),
                phase_group_label: "1".into(),
                phase_name: "Bracket".into(),
                phase_order: 1,
                bracket_type: "DOUBLE_ELIMINATION".into(),
                slots: [seed_slot(s.slots.first()), seed_slot(s.slots.get(1))],
            })
            .collect();
        let bracket = Bracket {
            event_name: seed.event_name,
            tournament_name: seed.tournament_name,
            stations: seed.stations,
            streams: seed.streams,
            sets,
        };
        let mut st = State {
            group: layout::groups_of(&bracket).into_iter().next(),
            selected: seed.selected,
            bracket: Some(bracket),
            ..State::default()
        };
        st.seed_pickers();
        st
    }

    fn ctx(blocked: Option<&str>) -> Ctx {
        Ctx {
            now_s: 1_786_000_000,
            blocked: blocked.map(str::to_string),
        }
    }

    #[test]
    fn view_renders_the_tree_and_the_action_bar() {
        // Exercises the whole widget tree headlessly — catches view() panics
        // and proves the cards and the selected set's actions reach the screen.
        let st = seeded();
        let mut ui = iced_test::simulator(screen(&st, &ctx(None)));
        assert!(ui.find("Bracket").is_ok());
        assert!(ui.find("The Hangout #47 · Rivals 2 Singles").is_ok());
        assert!(ui.find("Winners Quarter-Final").is_ok());
        assert!(
            ui.find("jugeeya").is_ok(),
            "a played set shows its entrants"
        );
        assert!(
            ui.find("Winners Semi-Final · BRUJITA vs NAVI").is_ok(),
            "the selected set's action bar is open"
        );
        assert!(
            ui.find("Re-call").is_ok(),
            "an ongoing set offers a re-call, not a fresh start"
        );
    }

    #[test]
    fn an_unseeded_set_offers_no_actions() {
        let mut st = seeded();
        st.selected = Some("w-i".into());
        let mut ui = iced_test::simulator(screen(&st, &ctx(None)));
        assert!(ui.find("Waiting on an earlier round.").is_ok());
        assert!(
            ui.find("Re-call").is_err() && ui.find("Start match").is_err(),
            "nothing to call when a seat is still empty"
        );
    }

    /// Both halves of a destination are independent — a set can be at a
    /// station AND on a stream — and both pickers seed from where start.gg
    /// already has the set, so acting without touching them moves nothing.
    #[test]
    fn both_pickers_seed_from_the_selected_set() {
        let mut st = seeded();
        // The live Semi-Final is on station 2, no stream.
        assert_eq!(st.picked_station.as_deref(), Some("Station 2"));
        assert_eq!(st.picked_stream, None);
        assert_eq!(st.station_choice(), Some(2));
        assert_eq!(st.stream_choice(), None);

        // A set that start.gg has on a stream seeds that half too.
        st.bracket.as_mut().unwrap().sets[1].stream = Some("socalrivals".into());
        st.seed_pickers();
        assert_eq!(st.picked_stream.as_deref(), Some("Stream: socalrivals"));
        assert_eq!(st.stream_choice().as_deref(), Some("socalrivals"));
    }

    /// "Leave it as is" has to mean "send nothing for that half", or every
    /// call would clobber the other one.
    #[test]
    fn leaving_a_half_alone_sends_nothing_for_it() {
        let mut st = seeded();
        st.picked_station = None;
        st.picked_stream = None;
        assert_eq!(st.station_choice(), None);
        assert_eq!(st.stream_choice(), None);
    }

    #[test]
    fn a_tournament_with_no_streams_still_renders_its_callable_set() {
        // The stream picker hides itself when there are no stream setups —
        // an empty dropdown is just a thing to wonder about. `pick_list`
        // content isn't reachable through `find`, so what this pins is that
        // both shapes render at all, either side of that branch.
        let mut st = seeded();
        assert!(!st.bracket.as_ref().unwrap().streams.is_empty());
        {
            let mut ui = iced_test::simulator(screen(&st, &ctx(None)));
            assert!(ui.find("Re-call").is_ok());
        }

        st.bracket.as_mut().unwrap().streams.clear();
        let mut ui = iced_test::simulator(screen(&st, &ctx(None)));
        assert!(ui.find("Re-call").is_ok());
    }

    #[test]
    fn a_reported_set_can_have_its_result_changed_in_two_clicks() {
        // The whole point: by top 8 the results worth fixing are on sets no
        // station recorded, so the console can't touch them — the bracket can.
        let mut st = seeded();
        st.selected = Some("w-c".into()); // completed, jugeeya beat Marsh
        {
            let mut ui = iced_test::simulator(screen(&st, &ctx(None)));
            assert!(ui.find("Reported — jugeeya advanced.").is_ok());
            assert!(ui.find("Change result").is_ok());
            assert!(
                ui.find("Cancel").is_err(),
                "resetting a reported set is never one stray tap away"
            );
        }

        st.changing_result = true;
        let mut ui = iced_test::simulator(screen(&st, &ctx(None)));
        assert!(
            ui.find("Marsh").is_ok(),
            "the seat that didn't win is the offer"
        );
        assert!(
            ui.find("Cancel").is_ok(),
            "and backing out doesn't require picking someone"
        );
        assert!(
            ui.find(
                "Resets the set on start.gg, then reports the new winner. Later rounds already \
                 played out of this set keep their results — fix those here too if they changed."
            )
            .is_ok(),
            "the one consequence that isn't undone for you is stated"
        );
    }

    #[test]
    fn a_station_install_sees_the_tree_but_cannot_act() {
        // Reading is unauthenticated, so the bracket itself still renders;
        // only the writes are withheld, and the reason is stated.
        let st = seeded();
        let mut ui = iced_test::simulator(screen(
            &st,
            &ctx(Some("Bracket actions run on the operator PC.")),
        ));
        assert!(ui.find("Winners Quarter-Final").is_ok());
        assert!(ui.find("Bracket actions run on the operator PC.").is_ok());
        assert!(ui.find("Start match").is_err());
        assert!(ui.find("Re-call").is_err());

        // Same for a reported set: the install-level block comes first, so
        // nothing offers to reset someone else's bracket.
        let mut done = seeded();
        done.selected = Some("w-c".into());
        let mut ui = iced_test::simulator(screen(
            &done,
            &ctx(Some("Bracket actions run on the operator PC.")),
        ));
        assert!(ui.find("Change result").is_err());
    }
}
