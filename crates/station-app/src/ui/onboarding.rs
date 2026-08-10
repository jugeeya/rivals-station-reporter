//! First-run setup — a faithful port of OnboardingView.vue: pick what this
//! PC is, then fill in only what can't be auto-detected, with the event URL
//! echoed back (tournament + entrants) so a wrong paste is caught here.

use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length, Task};

use super::{blocking, App, Message};
use crate::engine::commands;
use crate::model::{Config, EventSummary};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Station,
    Operator,
    Both,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Station => "station",
            Mode::Operator => "operator",
            Mode::Both => "both",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Mode::Station => "Station",
            Mode::Operator => "Operator",
            Mode::Both => "Both",
        }
    }

    fn desc(self) -> &'static str {
        match self {
            Mode::Station => {
                "This PC runs Rivals 2 at an event. Watches the game and reports each set to the operator."
            }
            Mode::Operator => {
                "The TO machine. Runs the hub every station reports to, and is the only PC that talks to start.gg."
            }
            Mode::Both => "One PC doing both jobs: plays games and runs the hub.",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    PickMode(Mode),
    Continue,
    Back,
    Station(String),
    EventUrl(String),
    CheckEvent,
    EventResolved(Result<EventSummary, String>),
    Key(String),
    Broker(String),
    Token(String),
    Finish,
    Saved(Result<(), String>),
    Paths(serde_json::Value),
}

pub struct State {
    step: u8,
    mode: Mode,
    station: String,
    event_url: String,
    event_info: Option<EventSummary>,
    event_error: String,
    resolving: bool,
    key: String,
    broker: String,
    token: String,
    saving: bool,
    save_error: String,
    save_path: String,
    save_exists: bool,
    replays_path: String,
    replays_exists: bool,
}

impl State {
    pub fn new(cfg: &Config) -> Self {
        let paths = commands::default_paths();
        Self {
            step: 1,
            mode: Mode::Station,
            station: "1".into(),
            event_url: String::new(),
            event_info: None,
            event_error: String::new(),
            resolving: false,
            key: String::new(),
            broker: cfg.broker.clone(),
            token: String::new(),
            saving: false,
            save_error: String::new(),
            save_path: paths["save"].as_str().unwrap_or_default().to_string(),
            save_exists: paths["saveExists"].as_bool().unwrap_or(false),
            replays_path: paths["replays"].as_str().unwrap_or_default().to_string(),
            replays_exists: paths["replaysExists"].as_bool().unwrap_or(false),
        }
    }

    fn is_station(&self) -> bool {
        self.mode != Mode::Operator
    }

    fn is_operator(&self) -> bool {
        self.mode != Mode::Station
    }
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    let ob = &mut app.onboarding;
    match msg {
        Msg::PickMode(m) => ob.mode = m,
        Msg::Continue => ob.step = 2,
        Msg::Back => ob.step = 1,
        Msg::Station(s) => ob.station = s,
        Msg::EventUrl(s) => {
            ob.event_url = s;
            ob.event_info = None;
            ob.event_error.clear();
        }
        Msg::CheckEvent => {
            let url = ob.event_url.trim().to_string();
            if url.is_empty() {
                return Task::none();
            }
            ob.resolving = true;
            return Task::perform(
                blocking(move || {
                    commands::resolve_event(&url).and_then(|v| {
                        serde_json::from_value::<EventSummary>(v).map_err(|e| e.to_string())
                    })
                }),
                |r| Message::Onboarding(Msg::EventResolved(r)),
            );
        }
        Msg::EventResolved(r) => {
            ob.resolving = false;
            match r {
                Ok(info) => ob.event_info = Some(info),
                Err(e) => ob.event_error = e,
            }
        }
        Msg::Key(s) => ob.key = s,
        Msg::Broker(s) => ob.broker = s,
        Msg::Token(s) => ob.token = s,
        Msg::Finish => {
            ob.saving = true;
            ob.save_error.clear();
            let cfg = Config {
                mode: ob.mode.as_str().to_string(),
                station: ob.station.trim().parse().unwrap_or(1),
                // A pasted URL must survive even when the user never pressed
                // Check (or the echo failed on venue wifi): fall back to
                // normalizing the raw URL instead of silently dropping it —
                // which left operators staring at an empty bracket panel.
                slug: ob
                    .event_info
                    .as_ref()
                    .map(|e| e.slug.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        let raw = ob.event_url.trim();
                        if raw.is_empty() {
                            String::new()
                        } else {
                            station_core::forwarder::normalize_slug(raw)
                        }
                    }),
                broker: if ob.broker.trim().is_empty() {
                    app.st.config.broker.clone()
                } else {
                    ob.broker.trim().to_string()
                },
                key: ob.key.trim().to_string(),
                startgg_token: ob.token.trim().to_string(),
                configured: true,
                ..app.st.config.clone()
            };
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || commands::save_config(&engine, cfg).map(|_| ())),
                |r| Message::Onboarding(Msg::Saved(r)),
            );
        }
        Msg::Saved(r) => {
            ob.saving = false;
            if let Err(e) = r {
                // Without this the button just flips back to "Start" and the
                // user is stuck on onboarding with no idea the save failed.
                ob.save_error = e;
            }
        }
        Msg::Paths(_) => {}
    }
    Task::none()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let ob = &app.onboarding;
    let inner: Element<'_, Message> = if ob.step == 1 {
        step_one(ob)
    } else {
        step_two(ob)
    };

    container(inner)
        .style(theme::card_rich)
        .padding(32)
        .width(Length::Fixed(640.0))
        .into()
}

fn step_one(ob: &State) -> Element<'_, Message> {
    let mode_card = |m: Mode| {
        let active = ob.mode == m;
        button(
            column![
                text(m.title())
                    .font(theme::FONT_DISPLAY)
                    .size(15)
                    .color(theme::TEXT_PRIMARY),
                text(m.desc()).size(12).color(theme::TEXT_MUTED),
            ]
            .spacing(6),
        )
        .style(move |t, status| {
            let mut s = theme::button_surface(t, status);
            if active {
                s.border.color = iced::Color {
                    a: 0.8,
                    ..theme::ACCENT
                };
            }
            s
        })
        .padding(14)
        .width(Length::Fill)
        .on_press(Message::Onboarding(Msg::PickMode(m)))
    };

    column![
        text("Rivals Station Reporter")
            .font(theme::FONT_DISPLAY)
            .size(26)
            .color(theme::TEXT_PRIMARY),
        text("What is this PC at your event?")
            .size(14)
            .color(theme::TEXT_MUTED),
        row![
            mode_card(Mode::Station),
            mode_card(Mode::Operator),
            mode_card(Mode::Both)
        ]
        .spacing(10),
        button(
            text("Continue")
                .size(15)
                .width(Length::Fill)
                .align_x(Alignment::Center)
        )
        .style(theme::button_primary_rich)
        .padding([10, 0])
        .width(Length::Fill)
        .on_press(Message::Onboarding(Msg::Continue)),
    ]
    .spacing(16)
    .into()
}

fn step_two(ob: &State) -> Element<'_, Message> {
    let label = |t: &'static str| text(t).size(13).color(theme::TEXT_PRIMARY);
    let opt = |t: &'static str| text(t).size(12).color(theme::TEXT_MUTED);
    let help = |t: String| text(t).size(12).color(theme::TEXT_MUTED);

    let mut col = column![text("Set it up")
        .font(theme::FONT_DISPLAY)
        .size(26)
        .color(theme::TEXT_PRIMARY)]
    .spacing(14);

    if ob.is_station() {
        col = col.push(
            column![
                label("Station number"),
                text_input("1", &ob.station)
                    .style(theme::input)
                    .padding(9)
                    .width(Length::Fixed(120.0))
                    .on_input(|s| Message::Onboarding(Msg::Station(s))),
                help("The start.gg station this setup is assigned to.".into()),
            ]
            .spacing(6),
        );
    }

    let mut check = button(text(if ob.resolving { "…" } else { "Check" }).size(13))
        .style(theme::button_surface)
        .padding([9, 14]);
    if !ob.resolving && !ob.event_url.trim().is_empty() {
        check = check.on_press(Message::Onboarding(Msg::CheckEvent));
    }
    let mut event_field = column![
        row![
            label("start.gg event"),
            opt("(optional; without one it's a local scoreboard)")
        ]
        .spacing(6),
        row![
            text_input("Paste a start.gg link…", &ob.event_url)
                .style(theme::input)
                .padding(9)
                .on_input(|s| Message::Onboarding(Msg::EventUrl(s)))
                .on_submit(Message::Onboarding(Msg::CheckEvent)),
            check
        ]
        .spacing(8),
    ]
    .spacing(6);
    if let Some(info) = &ob.event_info {
        let entrants = info
            .entrants
            .map(|n| format!(" · {n} entrants"))
            .unwrap_or_default();
        event_field = event_field.push(
            text(format!("✓ {} · {}{}", info.tournament, info.name, entrants))
                .size(12)
                .color(theme::TEXT_SUCCESS),
        );
    } else if !ob.event_error.is_empty() {
        event_field = event_field.push(
            text(ob.event_error.clone())
                .size(12)
                .color(theme::TEXT_FAILURE),
        );
    }
    col = col.push(event_field);

    if ob.mode == Mode::Station {
        col = col.push(
            column![
                label("Hub / broker URL"),
                text_input(
                    "http://192.168.…:8787 (from the operator's screen)",
                    &ob.broker
                )
                .style(theme::input)
                .padding(9)
                .on_input(|s| Message::Onboarding(Msg::Broker(s))),
                help(
                    "Shown big on the operator's screen, or leave the cloud broker default.".into()
                ),
            ]
            .spacing(6),
        );
    }

    let mut key_field = column![
        row![
            label("Shared key"),
            opt("(required to send; ask whoever runs the event)")
        ]
        .spacing(6),
        text_input("", &ob.key)
            .style(theme::input)
            .padding(9)
            .secure(true)
            .on_input(|s| Message::Onboarding(Msg::Key(s))),
    ]
    .spacing(6);
    if ob.is_operator() && ob.key.trim().is_empty() {
        key_field = key_field.push(
            text(
                "Without a key, anyone on the venue's network can post to this hub — \
                 including reporting sets to your bracket. Pick one and give it to your stations.",
            )
            .size(12)
            .color(theme::TEXT_WARNING),
        );
    }
    col = col.push(key_field);

    if ob.is_operator() {
        col = col.push(
            column![
                row![
                    label("start.gg API token"),
                    opt("(operator only; stays on this machine)")
                ]
                .spacing(6),
                text_input("", &ob.token)
                    .style(theme::input)
                    .padding(9)
                    .secure(true)
                    .on_input(|s| Message::Onboarding(Msg::Token(s))),
            ]
            .spacing(6),
        );
    }

    if ob.is_station() {
        let detect_row = |ok: bool, name: &'static str, path: &str| {
            row![
                text(if ok { "✓" } else { "⚠" }).size(13).color(if ok {
                    theme::TEXT_SUCCESS
                } else {
                    theme::TEXT_WARNING
                }),
                text(name).size(12).color(theme::TEXT_PRIMARY),
                text(path.to_string()).size(12).color(theme::TEXT_MUTED),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
        };
        let mut detect = column![
            detect_row(ob.save_exists, "Stats save", &ob.save_path),
            detect_row(ob.replays_exists, "Replays", &ob.replays_path),
        ]
        .spacing(6);
        if !ob.save_exists {
            detect = detect.push(
                text("Save not found. Has Rivals 2 been run on this PC? (Paths can be changed later in Settings.)")
                    .size(12)
                    .color(theme::TEXT_WARNING),
            );
        }
        col = col.push(
            container(detect)
                .style(theme::panel)
                .padding(12)
                .width(Length::Fill),
        );
    }

    if !ob.save_error.is_empty() {
        col = col.push(
            text(ob.save_error.clone())
                .size(12)
                .color(theme::TEXT_FAILURE),
        );
    }

    let mut start = button(text(if ob.saving { "Starting…" } else { "Start" }).size(15))
        .style(theme::button_primary_rich)
        .padding([10, 28]);
    if !ob.saving {
        start = start.on_press(Message::Onboarding(Msg::Finish));
    }

    col = col.push(
        row![
            button(text("← Back").size(13))
                .style(theme::button_linkish)
                .on_press(Message::Onboarding(Msg::Back)),
            Space::new().width(Length::Fill),
            start
        ]
        .align_y(Alignment::Center),
    );

    col.into()
}
