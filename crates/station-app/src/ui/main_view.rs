//! The one screen after setup — port of MainView.vue: header, health strip,
//! hub banner, then the station widgets (live set card + recent sets) and/or
//! the operator console + Current Sets, by mode.

use iced::widget::{button, column, container, row, scrollable, text, tooltip, Space};
use iced::{Alignment, Element, Length};

use super::{console, current_sets, format, App, Message};
use crate::theme;

pub fn view(app: &App) -> Element<'_, Message> {
    let cfg = &app.st.config;
    let is_station = cfg.mode != "operator";
    let is_operator = cfg.mode != "station";

    // ---- header --------------------------------------------------------------
    let mut title_row = row![text("Rivals Station Reporter")
        .font(theme::FONT_DISPLAY)
        .size(20)
        .color(theme::TEXT_PRIMARY)]
    .spacing(10)
    .align_y(Alignment::Center);
    if is_station {
        title_row = title_row.push(
            text(format!("Station {}", cfg.station))
                .size(12)
                .color(theme::TEXT_MUTED),
        );
    }
    title_row = title_row.push(
        text(cfg.mode.to_uppercase())
            .size(11)
            .color(theme::TEXT_MUTED),
    );
    if cfg.dry_run {
        title_row = title_row.push(text("DRY-RUN").size(11).color(theme::TEXT_WARNING));
    }

    let actions = row![
        button(text(if app.show_log { "Log ▾" } else { "Log" }).size(13))
            .style(theme::button_linkish)
            .on_press(Message::ToggleLog),
        button(text("Settings").size(13))
            .style(theme::button_linkish)
            .on_press(Message::OpenSettings),
    ]
    .spacing(4);

    let header = row![title_row, Space::new().width(Length::Fill), actions]
        .align_y(Alignment::Center);

    // ---- health strip ----------------------------------------------------------
    let mut chips = row![].spacing(6);
    let h = &app.st.health;
    if is_station {
        chips = chips.push(chip(
            "Save",
            h.save_armed,
            false,
            if h.save_armed {
                format!("Watching {}", h.save_path)
            } else if h.save_exists {
                "Save found but not read yet".into()
            } else {
                "Save not found. Has Rivals 2 been run on this PC? Fix the path in Settings.".into()
            },
        ));
        chips = chips.push(chip(
            "Replays",
            h.replays_exists,
            false,
            if h.replays_exists {
                h.replays_path.clone()
            } else {
                "Replays folder not found. Timestamps and slots degrade without it.".into()
            },
        ));
        let sending_ok =
            !cfg.slug.is_empty() && (!cfg.broker.is_empty() || app.st.hub_url.is_some());
        chips = chips.push(chip(
            "Sending",
            sending_ok,
            cfg.slug.is_empty(),
            if !cfg.slug.is_empty() {
                format!(
                    "Reporting to {}",
                    app.st.hub_url.clone().unwrap_or_else(|| cfg.broker.clone())
                )
            } else {
                "No event configured. Local scoreboard only, nothing is sent.".into()
            },
        ));
    }
    if is_operator {
        chips = chips.push(chip(
            "Hub",
            app.st.hub_url.is_some(),
            false,
            app.st
                .hub_url
                .clone()
                .map(|u| format!("Serving {u}"))
                .unwrap_or_else(|| "Hub not running. Check the log.".into()),
        ));
        chips = chips.push(chip(
            "start.gg",
            !cfg.startgg_token.is_empty(),
            cfg.startgg_token.is_empty(),
            if !cfg.startgg_token.is_empty() {
                "Token configured. Live scores and reports go to the bracket.".into()
            } else {
                "No API token. Sets are tracked but nothing reaches start.gg.".into()
            },
        ));
    }

    // ---- body ------------------------------------------------------------------
    let mut body = column![].spacing(14);

    if is_operator {
        if let Some(url) = &app.st.hub_url {
            body = body.push(
                container(
                    row![
                        text("Stations point here:").size(13).color(theme::TEXT_MUTED),
                        text(url.clone())
                            .font(theme::FONT_MONO)
                            .size(16)
                            .color(theme::TEXT_PRIMARY),
                        Space::new().width(Length::Fill),
                        button(text("Copy").size(12))
                            .style(theme::button_surface)
                            .padding([6, 12])
                            .on_press(Message::CopyHubUrl),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .style(theme::panel)
                .padding([10, 14])
                .width(Length::Fill),
            );
        }
    }

    if is_station {
        body = body.push(live_set_card(app));
        body = body.push(station_sets(app));
    }

    if is_operator {
        body = body.push(console::view(app));
        body = body.push(current_sets::view(app));
    }

    // ---- footer ----------------------------------------------------------------
    let status = &app.st.status;
    let d = (app.now_s - status.t).max(0);
    let ago = if d < 2 {
        "just now".to_string()
    } else if d < 60 {
        format!("{d}s ago")
    } else {
        format!("{}m ago", d / 60)
    };
    // The status text takes whatever room the slug doesn't need and clips
    // (never wraps into a tall footer, never pushes the slug off-screen).
    let footer = row![
        text("●").size(10).color(if status.error {
            theme::TEXT_FAILURE
        } else {
            theme::TEXT_SUCCESS
        }),
        container(
            text(format!("{} · {}", status.msg, ago))
                .size(12)
                .color(theme::TEXT_MUTED)
                .wrapping(iced::widget::text::Wrapping::None)
        )
        .width(Length::Fill)
        .clip(true),
        text(cfg.slug.clone())
            .size(12)
            .color(theme::TEXT_MUTED)
            .wrapping(iced::widget::text::Wrapping::None),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        column![
            header,
            chips,
            scrollable(body)
                .id("main-body")
                .height(Length::Fill)
                .spacing(4),
            footer
        ]
        .spacing(14),
    )
    .style(theme::card)
    .padding(24)
    .width(Length::Fixed(920.0))
    .height(Length::Fill)
    .into()
}

fn chip<'a>(label: &'a str, ok: bool, warn: bool, detail: String) -> Element<'a, Message> {
    let (icon, icon_color) = if ok {
        ("✓", theme::TEXT_SUCCESS)
    } else if warn {
        ("…", theme::TEXT_WARNING)
    } else {
        ("⚠", theme::TEXT_FAILURE)
    };
    let chip = container(
        row![
            text(icon).size(12).color(icon_color),
            text(label).size(12).color(if warn {
                theme::TEXT_MUTED
            } else {
                theme::TEXT_PRIMARY
            }),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .style(theme::panel)
    .padding([4, 10]);

    tooltip(chip, container(text(detail).size(12)).style(theme::tooltip_bubble).padding(8), tooltip::Position::Bottom)
        .into()
}

// ---- station widgets -----------------------------------------------------------

/// Port of LiveSetCard.vue: the set being played right now, front and center.
fn live_set_card(app: &App) -> Element<'_, Message> {
    let Some(live) = &app.st.snapshot.live else {
        return container(
            text("Waiting for a game…").size(14).color(theme::TEXT_MUTED),
        )
        .style(theme::panel)
        .padding(24)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into();
    };

    let reportable = live
        .mode
        .as_deref()
        .map(|m| m.eq_ignore_ascii_case("local"))
        .unwrap_or(true);

    let mut head = row![
        text("●").size(11).color(theme::ACCENT),
        text("Now playing").size(12).color(theme::TEXT_MUTED),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if !reportable {
        if let Some(m) = &live.mode {
            head = head.push(
                text(format!("{}: not a bracket set", m.to_lowercase()))
                    .size(11)
                    .color(theme::TEXT_WARNING),
            );
        }
    }
    head = head.push(Space::new().width(Length::Fill));
    head = head.push(
        text(format!(
            "{} game{}",
            live.games,
            if live.games == 1 { "" } else { "s" }
        ))
        .size(12)
        .color(theme::TEXT_MUTED),
    );

    let player = |i: usize| -> Element<'_, Message> {
        let Some(p) = live.players.get(i) else {
            return Space::new().into();
        };
        // The set leader's tag reads green (web .ls-player--lead .ls-tag);
        // the other player stays plain white, not dimmed.
        let mut tag_row = row![text(p.tag.clone())
            .font(theme::FONT_DISPLAY)
            .size(22)
            .color(if p.won { theme::TEXT_SUCCESS } else { theme::TEXT_PRIMARY })]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(sgg) = &p.sgg {
            tag_row = tag_row.push(
                tooltip(
                    text(format!("@{sgg}")).size(12).color(theme::TEXT_MUTED),
                    container(text(format::sgg_title(&p.tag, sgg)).size(12))
                        .style(theme::tooltip_bubble)
                        .padding(8)
                        .max_width(360),
                    tooltip::Position::Bottom,
                ),
            );
        }
        column![tag_row, text(p.character.clone()).size(13).color(theme::TEXT_MUTED)]
            .spacing(2)
            .into()
    };

    let score = row![
        text(
            live.players
                .first()
                .map(|p| p.wins.to_string())
                .unwrap_or_default()
        )
        .font(theme::FONT_DISPLAY)
        .size(34)
        .color(theme::TEXT_PRIMARY),
        text("–").size(28).color(theme::TEXT_MUTED),
        text(
            live.players
                .get(1)
                .map(|p| p.wins.to_string())
                .unwrap_or_default()
        )
        .font(theme::FONT_DISPLAY)
        .size(34)
        .color(theme::TEXT_PRIMARY),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let players_row = row![
        player(0),
        Space::new().width(Length::Fill),
        score,
        Space::new().width(Length::Fill),
        player(1)
    ]
    .align_y(Alignment::Center);

    let mut col = column![head, players_row].spacing(12);
    if !reportable {
        col = col.push(
            text("Shown here, kept out of start.gg.")
                .size(12)
                .color(theme::TEXT_MUTED),
        );
    }

    container(col)
        .style(theme::panel)
        .padding(18)
        .width(Length::Fill)
        .into()
}

/// Port of StationSets.vue: recent sets on THIS station, newest first.
fn station_sets(app: &App) -> Element<'_, Message> {
    let mut sets: Vec<_> = app.st.snapshot.history.iter().collect();
    sets.reverse();

    let mut col = column![row![
        text("Sets today").size(12).color(theme::TEXT_MUTED),
        text(if sets.is_empty() {
            String::new()
        } else {
            sets.len().to_string()
        })
        .size(12)
        .color(theme::TEXT_MUTED),
    ]
    .spacing(6)]
    .spacing(6);

    if sets.is_empty() {
        return col
            .push(text("Finished sets will appear here.").size(13).color(theme::TEXT_MUTED))
            .into();
    }

    for s in sets {
        let reportable = s
            .mode
            .as_deref()
            .map(|m| m.eq_ignore_ascii_case("local"))
            .unwrap_or(true);
        let mut r = row![text(format::clock(s.start_epoch))
            .font(theme::FONT_MONO)
            .size(12)
            .color(theme::TEXT_MUTED)]
        .spacing(10)
        .align_y(Alignment::Center);

        let mut players = row![].spacing(6).align_y(Alignment::Center);
        for (j, p) in s.players.iter().enumerate() {
            if j > 0 {
                players = players.push(text("vs").size(11).color(theme::TEXT_MUTED));
            }
            // Winner tags are green (web .ss-tag--won) — except in greyed
            // online-ladder rows, where the win never mattered to a bracket
            // and green would overstate it.
            players = players.push(
                text(p.tag.clone())
                    .size(13)
                    .color(if p.won && reportable {
                        theme::TEXT_SUCCESS
                    } else if reportable {
                        theme::TEXT_PRIMARY
                    } else {
                        theme::TEXT_MUTED
                    }),
            );
            players = players.push(
                text(format!("({})", p.character)).size(11).color(theme::TEXT_MUTED),
            );
        }
        r = r.push(players);
        r = r.push(Space::new().width(Length::Fill));
        r = r.push(
            text(
                s.players
                    .iter()
                    .map(|p| p.wins.to_string())
                    .collect::<Vec<_>>()
                    .join("–"),
            )
            .font(theme::FONT_MONO)
            .size(13)
            .color(theme::TEXT_PRIMARY),
        );
        if !reportable {
            if let Some(m) = &s.mode {
                r = r.push(text(m.to_lowercase()).size(11).color(theme::TEXT_WARNING));
            }
        }
        col = col.push(container(r).padding([6, 8]).width(Length::Fill));
    }

    col.into()
}
