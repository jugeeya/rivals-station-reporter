//! Settings drawer — port of SettingsDrawer.vue. Edits a draft of the
//! config; nothing applies until Save (which rebuilds the engine).

use iced::widget::{button, checkbox, column, container, pick_list, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Task};

use super::{blocking, App, Message};
use crate::engine::commands;
use crate::model::Config;
use crate::theme;

#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    Mode(String),
    Station(String),
    Slug(String),
    Broker(String),
    Key(String),
    Token(String),
    Save(String),
    Replays(String),
    Dir(String),
    Idle(String),
    Poll(String),
    HubPort(String),
    DryRun(bool),
    PickSave,
    PickReplays,
    PickDir,
    Picked(PickTarget, Option<String>),
    Autostart(bool),
    AutostartDone(Result<bool, String>),
    DoSave,
    Saved(Result<(), String>),
    CheckUpdate,
    UpdateChecked(Result<Option<super::updater::Update>, String>),
    ApplyUpdate,
    UpdateApplied(Result<(), String>),
    RestartNow,
    ScanHubs,
    HubsFound(Vec<FoundHub>),
    UseHub(String),
}

/// One hub the LAN sweep found — url plus what its /health advertises.
#[derive(Debug, Clone)]
pub struct FoundHub {
    pub url: String,
    pub slug: Option<String>,
    pub startgg: bool,
}

#[derive(Default)]
enum UpdateFlow {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(super::updater::Update),
    Downloading(super::updater::Update),
    Staged,
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub enum PickTarget {
    Save,
    Replays,
    Dir,
}

pub struct State {
    mode: String,
    station: String,
    slug: String,
    broker: String,
    key: String,
    token: String,
    save: String,
    replays: String,
    dir: String,
    idle: String,
    poll: String,
    hub_port: String,
    dry_run: bool,
    autostart: bool,
    saving: bool,
    err: String,
    update: UpdateFlow,
    scanning: bool,
    scanned: bool,
    hubs: Vec<FoundHub>,
}

impl State {
    pub fn new(cfg: &Config) -> Self {
        Self {
            mode: cfg.mode.clone(),
            station: cfg.station.to_string(),
            slug: cfg.slug.clone(),
            broker: cfg.broker.clone(),
            key: cfg.key.clone(),
            token: cfg.startgg_token.clone(),
            save: cfg.save.clone(),
            replays: cfg.replays.clone(),
            dir: cfg.dir.clone(),
            idle: cfg.idle.to_string(),
            poll: cfg.poll.to_string(),
            hub_port: cfg.hub_port.to_string(),
            dry_run: cfg.dry_run,
            autostart: commands::get_autostart().unwrap_or(false),
            saving: false,
            err: String::new(),
            update: UpdateFlow::Idle,
            scanning: false,
            scanned: false,
            hubs: Vec::new(),
        }
    }
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    let Some(s) = app.settings.as_mut() else {
        return Task::none();
    };
    match msg {
        Msg::Close => {
            app.settings = None;
        }
        Msg::Mode(v) => s.mode = v,
        Msg::Station(v) => s.station = v,
        Msg::Slug(v) => s.slug = v,
        Msg::Broker(v) => s.broker = v,
        Msg::Key(v) => s.key = v,
        Msg::Token(v) => s.token = v,
        Msg::Save(v) => s.save = v,
        Msg::Replays(v) => s.replays = v,
        Msg::Dir(v) => s.dir = v,
        Msg::Idle(v) => s.idle = v,
        Msg::Poll(v) => s.poll = v,
        Msg::HubPort(v) => s.hub_port = v,
        Msg::DryRun(v) => s.dry_run = v,
        Msg::PickSave => {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose Rivals2_StatsSaveSlot.sav")
                        .add_filter(".sav file", &["sav"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_string_lossy().into_owned())
                },
                |p| Message::Settings(Msg::Picked(PickTarget::Save, p)),
            );
        }
        Msg::PickReplays => {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose folder")
                        .pick_folder()
                        .await
                        .map(|f| f.path().to_string_lossy().into_owned())
                },
                |p| Message::Settings(Msg::Picked(PickTarget::Replays, p)),
            );
        }
        Msg::PickDir => {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Choose folder")
                        .pick_folder()
                        .await
                        .map(|f| f.path().to_string_lossy().into_owned())
                },
                |p| Message::Settings(Msg::Picked(PickTarget::Dir, p)),
            );
        }
        Msg::Picked(target, Some(path)) => match target {
            PickTarget::Save => s.save = path,
            PickTarget::Replays => s.replays = path,
            PickTarget::Dir => s.dir = path,
        },
        Msg::Picked(_, None) => {}
        Msg::Autostart(v) => {
            // Applied optimistically; reverted on failure (AutostartDone).
            s.autostart = v;
            return Task::perform(
                blocking(move || commands::set_autostart(v).map(|_| v)),
                |r| Message::Settings(Msg::AutostartDone(r)),
            );
        }
        Msg::AutostartDone(r) => {
            if let Err(e) = r {
                s.autostart = !s.autostart;
                s.err = e;
            }
        }
        Msg::DoSave => {
            s.saving = true;
            s.err.clear();
            let cfg = Config {
                mode: s.mode.clone(),
                station: s.station.trim().parse().unwrap_or(1),
                // Accept a full start.gg URL here too — saved as the bare
                // slug the API actually wants.
                slug: station_core::forwarder::normalize_slug(s.slug.trim()),
                broker: s.broker.trim().to_string(),
                key: s.key.trim().to_string(),
                startgg_token: s.token.trim().to_string(),
                save: s.save.trim().to_string(),
                replays: s.replays.trim().to_string(),
                dir: s.dir.trim().to_string(),
                idle: s.idle.trim().parse::<f64>().unwrap_or(420.0).clamp(30.0, 3600.0),
                poll: s.poll.trim().parse::<f64>().unwrap_or(2.0).clamp(0.5, 60.0),
                hub_port: s.hub_port.trim().parse().unwrap_or(8787),
                dry_run: s.dry_run,
                configured: true,
            };
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || commands::save_config(&engine, cfg).map(|_| ())),
                |r| Message::Settings(Msg::Saved(r)),
            );
        }
        Msg::Saved(r) => {
            s.saving = false;
            match r {
                Ok(()) => app.settings = None,
                Err(e) => s.err = e,
            }
        }
        Msg::CheckUpdate => {
            s.update = UpdateFlow::Checking;
            return Task::perform(blocking(super::updater::check), |r| {
                Message::Settings(Msg::UpdateChecked(r))
            });
        }
        Msg::UpdateChecked(r) => {
            s.update = match r {
                Ok(Some(u)) => UpdateFlow::Available(u),
                Ok(None) => UpdateFlow::UpToDate,
                Err(e) => UpdateFlow::Failed(e),
            };
        }
        Msg::ApplyUpdate => {
            if let UpdateFlow::Available(u) = &s.update {
                let u = u.clone();
                s.update = UpdateFlow::Downloading(u.clone());
                return Task::perform(
                    blocking(move || super::updater::apply(&u)),
                    |r| Message::Settings(Msg::UpdateApplied(r)),
                );
            }
        }
        Msg::UpdateApplied(r) => {
            s.update = match r {
                Ok(()) => UpdateFlow::Staged,
                Err(e) => UpdateFlow::Failed(e),
            };
        }
        Msg::RestartNow => {
            if let Err(e) = super::updater::restart() {
                s.update = UpdateFlow::Failed(e);
            }
        }
        Msg::ScanHubs => {
            s.scanning = true;
            s.hubs.clear();
            let engine = app.engine.clone();
            return Task::perform(
                blocking(move || {
                    let found = commands::find_hubs(&engine);
                    found["hubs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|h| FoundHub {
                            url: h["url"].as_str().unwrap_or_default().to_string(),
                            slug: h["slug"].as_str().map(str::to_string),
                            startgg: h["startgg"].as_bool().unwrap_or(false),
                        })
                        .collect::<Vec<_>>()
                }),
                |hubs| Message::Settings(Msg::HubsFound(hubs)),
            );
        }
        Msg::HubsFound(hubs) => {
            s.scanning = false;
            s.scanned = true;
            // Exactly one hub on the LAN (the normal case): connect to it
            // without a second click, same as the web drawer did.
            if hubs.len() == 1 {
                s.broker = hubs[0].url.clone();
            }
            s.hubs = hubs;
        }
        Msg::UseHub(url) => {
            s.broker = url;
        }
    }
    Task::none()
}

pub fn view<'a>(_app: &'a App, s: &'a State) -> Element<'a, Message> {
    let label = |t: &'static str| text(t).size(12).color(theme::TEXT_MUTED);
    let field = |l: &'static str, input: Element<'a, Message>| {
        column![label(l), input].spacing(4)
    };
    let ti = |placeholder: &'a str, value: &'a str, f: fn(String) -> Msg| {
        text_input(placeholder, value)
            .style(theme::input)
            .padding(8)
            .size(13)
            .on_input(move |v| Message::Settings(f(v)))
    };

    let is_station = s.mode != "operator";
    let is_operator = s.mode != "station";

    let mut col = column![row![
        text("Settings").font(theme::FONT_DISPLAY).size(18).color(theme::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        button(text("✕").size(14))
            .style(theme::button_linkish)
            .on_press(Message::Settings(Msg::Close)),
    ]
    .align_y(Alignment::Center)]
    .spacing(12);

    col = col.push(field(
        "Mode",
        pick_list(
            vec!["station".to_string(), "operator".to_string(), "both".to_string()],
            Some(s.mode.clone()),
            |v| Message::Settings(Msg::Mode(v)),
        )
        .text_size(13)
        .padding([6, 10])
        .style(theme::pick_list_style)
        .menu_style(theme::pick_list_menu)
        .into(),
    ));

    if is_station {
        col = col.push(field("Station number", ti("1", &s.station, Msg::Station).into()));
    }
    col = col.push(field("start.gg event slug", ti("tournament/…/event/…", &s.slug, Msg::Slug).into()));
    if is_station {
        let mut hub_field = column![
            label("Hub / broker URL"),
            row![
                ti("http://…:8787", &s.broker, Msg::Broker),
                button(text(if s.scanning { "Scanning…" } else { "Find hub" }).size(12))
                    .style(theme::button_surface)
                    .padding([7, 10])
                    .on_press_maybe(
                        (!s.scanning).then_some(Message::Settings(Msg::ScanHubs))
                    ),
            ]
            .spacing(6),
        ]
        .spacing(4);
        for h in &s.hubs {
            let label_txt = format!(
                "{} · {}{}",
                h.url,
                h.slug.clone().unwrap_or_else(|| "no event configured".into()),
                if h.startgg { "" } else { ", no start.gg token" }
            );
            hub_field = hub_field.push(
                button(text(label_txt).size(11))
                    .style(theme::button_linkish)
                    .padding([2, 4])
                    .on_press(Message::Settings(Msg::UseHub(h.url.clone()))),
            );
        }
        if s.scanned && s.hubs.is_empty() {
            hub_field = hub_field.push(
                text("No hub found on this network.").size(11).color(theme::TEXT_MUTED),
            );
        }
        col = col.push(hub_field);
    }
    col = col.push(field(
        "Shared key",
        text_input("", &s.key)
            .style(theme::input)
            .padding(8)
            .size(13)
            .secure(true)
            .on_input(|v| Message::Settings(Msg::Key(v)))
            .into(),
    ));
    if is_operator {
        col = col.push(field(
            "start.gg API token",
            text_input("", &s.token)
                .style(theme::input)
                .padding(8)
                .size(13)
                .secure(true)
                .on_input(|v| Message::Settings(Msg::Token(v)))
                .into(),
        ));
        col = col.push(field("Hub port", ti("8787", &s.hub_port, Msg::HubPort).into()));
    }
    if is_station {
        let with_browse = |input: Element<'a, Message>, pick: Msg| {
            row![
                input,
                button(text("Browse").size(12))
                    .style(theme::button_surface)
                    .padding([7, 10])
                    .on_press(Message::Settings(pick))
            ]
            .spacing(6)
        };
        col = col.push(field(
            "Stats save (.sav)",
            with_browse(ti("auto-detect", &s.save, Msg::Save).into(), Msg::PickSave).into(),
        ));
        col = col.push(field(
            "Replays folder",
            with_browse(ti("auto-detect", &s.replays, Msg::Replays).into(), Msg::PickReplays)
                .into(),
        ));
        col = col.push(field(
            "Output folder",
            with_browse(ti("auto", &s.dir, Msg::Dir).into(), Msg::PickDir).into(),
        ));
        col = col.push(
            row![
                field("Idle timeout (s)", ti("420", &s.idle, Msg::Idle).into()),
                field("Poll (s)", ti("2", &s.poll, Msg::Poll).into()),
            ]
            .spacing(10),
        );
    }

    col = col.push(
        checkbox(s.dry_run)
            .label("Dry-run (log what would be sent, send nothing)")
            .text_size(13)
            .size(16)
            .on_toggle(|v| Message::Settings(Msg::DryRun(v))),
    );
    col = col.push(
        checkbox(s.autostart)
            .label("Start with the system")
            .text_size(13)
            .size(16)
            .on_toggle(|v| Message::Settings(Msg::Autostart(v))),
    );

    if !s.err.is_empty() {
        col = col.push(text(s.err.clone()).size(12).color(theme::TEXT_FAILURE));
    }

    let mut save = button(
        text(if s.saving { "Saving…" } else { "Save" })
            .size(14)
            .width(Length::Fill)
            .align_x(Alignment::Center),
    )
    .style(theme::button_primary_rich)
    .padding([9, 0])
    .width(Length::Fill);
    if !s.saving {
        save = save.on_press(Message::Settings(Msg::DoSave));
    }
    col = col.push(save);

    // ---- updates -------------------------------------------------------------
    let mut update_row = row![text(format!("v{}", env!("CARGO_PKG_VERSION")))
        .size(11)
        .color(theme::TEXT_MUTED)]
    .spacing(10)
    .align_y(Alignment::Center);
    match &s.update {
        UpdateFlow::Idle => {
            update_row = update_row.push(
                button(text("Check for updates").size(11))
                    .style(theme::button_linkish)
                    .padding([2, 4])
                    .on_press(Message::Settings(Msg::CheckUpdate)),
            );
        }
        UpdateFlow::Checking => {
            update_row = update_row.push(text("checking…").size(11).color(theme::TEXT_MUTED));
        }
        UpdateFlow::UpToDate => {
            update_row = update_row.push(text("up to date").size(11).color(theme::TEXT_SUCCESS));
        }
        UpdateFlow::Available(u) => {
            update_row = update_row.push(
                button(text(format!("Update to v{}", u.version)).size(11))
                    .style(theme::button_primary_rich)
                    .padding([3, 10])
                    .on_press(Message::Settings(Msg::ApplyUpdate)),
            );
        }
        UpdateFlow::Downloading(u) => {
            update_row = update_row.push(
                text(format!("downloading v{}…", u.version))
                    .size(11)
                    .color(theme::TEXT_MUTED),
            );
        }
        UpdateFlow::Staged => {
            update_row = update_row.push(
                button(text("Restart to finish update").size(11))
                    .style(theme::button_primary_rich)
                    .padding([3, 10])
                    .on_press(Message::Settings(Msg::RestartNow)),
            );
        }
        UpdateFlow::Failed(e) => {
            update_row = update_row.push(text(e.clone()).size(11).color(theme::TEXT_FAILURE));
        }
    }
    col = col.push(update_row);

    // Right padding leaves room for the scrollbar so fields don't run under
    // it (or past the panel edge).
    let panel = container(
        scrollable(col.padding(iced::Padding {
            top: 0.0,
            right: 16.0,
            bottom: 0.0,
            left: 2.0,
        }))
        .height(Length::Fill),
    )
    .style(theme::drawer)
    .padding(18)
    .width(Length::Fixed(440.0))
    .height(Length::Fill);

    container(row![Space::new().width(Length::Fill), panel])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
