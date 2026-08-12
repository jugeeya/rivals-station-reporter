//! The Iced application: one `App` holding the engine handle and the typed
//! state mirror, with each screen (onboarding, main view, console, Current
//! Sets, settings, log) in its own module carrying its own `Msg`/`State`/
//! `view` — the same decomposition the Vue components had.

pub mod bracket;
pub mod chars;
pub mod console;
pub mod current_sets;
pub mod format;
pub mod main_view;
pub mod onboarding;
pub mod settings;
pub mod tag_installer;
pub mod tray;
pub mod updater;
pub mod vod_splitter;

use std::sync::{Arc, Mutex};

use iced::futures::channel::mpsc as fmpsc;
use iced::futures::StreamExt;
use iced::widget::{center, container, stack};
use iced::{Element, Length, Subscription, Task};
use serde_json::Value;

use crate::engine::{self, commands, EngineInner};
use crate::model::EngineState;
use crate::{background, theme};

/// Run a blocking engine/hub call off the UI thread.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("blocking task panicked")
}

/// The top-right page actions both main views share: the Bracket/Matches
/// toggle, then the other screens and overlays. Same page, different view
/// piece — so nothing moves when the toggle is clicked.
pub fn nav_actions(app: &App, bracket_active: bool) -> Element<'_, Message> {
    use iced::widget::{button, row, text, Space};
    row![
        view_toggle(
            bracket_active,
            Message::OpenBracket,
            Message::Bracket(bracket::Msg::Close),
        ),
        Space::new().width(Length::Fixed(8.0)),
        button(text("Tag Installer").size(13))
            .style(theme::button_linkish)
            .on_press(Message::OpenTagInstaller),
        button(text("VOD Splitter").size(13))
            .style(theme::button_linkish)
            .on_press(Message::OpenVodSplitter),
        button(text(if app.show_log { "Log ▾" } else { "Log" }).size(13))
            .style(theme::button_linkish)
            .on_press(Message::ToggleLog),
        button(text("Settings").size(13))
            .style(theme::button_linkish)
            .on_press(Message::OpenSettings),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The Bracket View / Matches View toggle both main screens carry top-right.
/// The active half is inert (nothing to do), the other switches screens.
pub fn view_toggle<'a, M: Clone + 'a>(
    bracket_active: bool,
    to_bracket: M,
    to_matches: M,
) -> iced::Element<'a, M> {
    use iced::widget::{button, row, text};
    let half = |label: &'static str, active: bool, msg: M| {
        button(text(label).size(13))
            .style(if active {
                theme::button_tab_active
            } else {
                theme::button_surface
            })
            .padding([5, 12])
            .on_press_maybe((!active).then_some(msg))
    };
    row![
        half("Bracket View", bracket_active, to_bracket),
        half("Matches View", !bracket_active, to_matches),
    ]
    .spacing(4)
    .into()
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A full engine state snapshot arrived on the feed.
    EngineState(Value),
    /// Slow clock for "ago"/elapsed labels.
    Tick,
    /// Operator background refresh of Current Sets.
    SetsAutoRefresh,
    Onboarding(onboarding::Msg),
    Bracket(bracket::Msg),
    OpenBracket,
    Console(console::Msg),
    Sets(current_sets::Msg),
    Settings(settings::Msg),
    Vod(vod_splitter::Msg),
    OpenVodSplitter,
    Tags(tag_installer::Msg),
    OpenTagInstaller,
    OpenSettings,
    ToggleLog,
    CopyHubUrl,
    /// The user clicked the window's close button.
    CloseRequested(iced::window::Id),
    /// Poll tick for tray menu events.
    TrayPoll,
    /// Dev screenshot loop (RSR_SCREENSHOT=path).
    ScreenshotTick,
    Screenshotted(Option<iced::window::Screenshot>),
}

/// Carrier for the engine feed receiver: `Subscription::run_with` needs a
/// `Hash + 'static` value and a capture-free builder fn, so the receiver
/// rides inside an Arc<Mutex<Option<..>>> that the builder takes from once.
/// The constant hash is correct: there is exactly one engine feed per app.
#[derive(Clone)]
pub struct Feed(pub Arc<Mutex<Option<fmpsc::UnboundedReceiver<Value>>>>);

impl std::hash::Hash for Feed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "engine-feed".hash(state);
    }
}

/// Which full-screen view is showing. Settings and the log stay overlays;
/// the VOD Splitter and Tag Installer are real second screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Reporter,
    Bracket,
    VodSplitter,
    TagInstaller,
}

pub struct App {
    pub engine: Arc<EngineInner>,
    feed: Feed,
    /// Typed mirror of the engine state (the web app's `state.s`).
    pub st: EngineState,
    /// First snapshot received — nothing renders before it.
    pub ready: bool,
    pub screen: Screen,
    pub onboarding: onboarding::State,
    pub bracket: bracket::State,
    pub console: console::State,
    pub current_sets: current_sets::State,
    pub vod: vod_splitter::State,
    pub tag_installer: tag_installer::State,
    pub settings: Option<settings::State>,
    pub show_log: bool,
    pub now_s: i64,
    /// RSR_SEED_STATE runs: fixture data must not be overwritten by live
    /// refreshes, so background Current Sets pulls are suppressed.
    frozen: bool,
    tray: Option<tray::Tray>,
    started: std::time::Instant,
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        // Same identifier-based directory the Tauri build used, so an
        // existing install keeps its config, learned tags, and hub state.
        // RSR_CONFIG_DIR overrides for dev/screenshot runs against a scratch
        // profile instead of the real one.
        let config_dir = std::env::var_os("RSR_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .expect("no config dir on this platform")
                    .join("io.github.jugeeya.rivals-station-reporter")
            });
        let engine::Engine(inner) = engine::start(config_dir.clone());

        let (tx, rx) = fmpsc::unbounded();
        inner.set_emitter(Box::new(move |v| {
            let _ = tx.unbounded_send(v);
        }));

        let st = EngineState::from_value(&commands::get_state(&inner));
        let onboarding = onboarding::State::new(&st.config);
        let configured_operator = st.config.configured && st.config.mode != "station";

        let mut app = Self {
            engine: inner,
            feed: Feed(Arc::new(Mutex::new(Some(rx)))),
            st,
            ready: true,
            screen: Screen::Reporter,
            onboarding,
            bracket: bracket::State::default(),
            console: console::State::default(),
            current_sets: current_sets::State::default(),
            vod: vod_splitter::State::new(config_dir),
            tag_installer: tag_installer::State::default(),
            settings: None,
            show_log: false,
            now_s: station_core::now_sec(),
            frozen: std::env::var_os("RSR_SEED_STATE").is_some(),
            tray: tray::create(),
            started: std::time::Instant::now(),
        };

        // Screenshot/dev hooks: seed fixture data (RSR_SEED_STATE=file.json
        // with optional `snapshot`/`hubSnapshot` keys) and open a drawer
        // (RSR_OPEN=settings|log) so automated captures can reach every
        // screen without a live tournament.
        let mut seed_task = Task::none();
        if let Some(path) = std::env::var_os("RSR_SEED_STATE") {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    app.engine.seed_dev_state(
                        v.get("snapshot").cloned(),
                        v.get("hubSnapshot").cloned(),
                        v.get("health").cloned(),
                        v.get("status").cloned(),
                    );
                    app.st = EngineState::from_value(&commands::get_state(&app.engine));
                    // Not part of EngineState in the real app either — it's
                    // the list_available_sets query; seeded straight into the
                    // panel's own state, same as the old capture harness did.
                    if let Some(av) = v.get("availableSets") {
                        if let Ok(data) = serde_json::from_value(av.clone()) {
                            app.current_sets.data = data;
                            app.current_sets.loaded = true;
                        }
                    }
                    // VOD Splitter fixture — its own screen, its own seed.
                    if let Some(vs) = v.get("vodSplitter") {
                        if let Ok(seed) = serde_json::from_value::<vod_splitter::Seed>(vs.clone()) {
                            seed_task = vod_splitter::apply_seed(&mut app, seed);
                        }
                    }
                    // Bracket fixture — that screen otherwise reads live.
                    if let Some(bs) = v.get("bracket") {
                        if let Ok(seed) = serde_json::from_value::<bracket::Seed>(bs.clone()) {
                            bracket::apply_seed(&mut app, seed);
                        }
                    }
                    // Tag Installer fixture — display-only, no network.
                    if let Some(ts) = v.get("tagInstaller") {
                        if let Ok(seed) = serde_json::from_value::<tag_installer::Seed>(ts.clone())
                        {
                            tag_installer::apply_seed(&mut app, seed);
                        }
                    }
                }
            }
        }
        match std::env::var("RSR_OPEN").as_deref() {
            Ok("settings") => {
                app.settings = Some(settings::State::new(&app.st.config, &app.engine))
            }
            Ok("log") => app.show_log = true,
            Ok("bracket") => app.screen = Screen::Bracket,
            Ok("vod") => app.screen = Screen::VodSplitter,
            Ok("tags") => app.screen = Screen::TagInstaller,
            _ => {}
        }

        // An operator's home screen is the bracket — the thing a TO is
        // actually running — with Matches View a toggle away. A station stays
        // on its own screen (health chips, the live set: the reasons that PC
        // runs this app at all). Never under a seed, whose fixture belongs to
        // whatever screen RSR_OPEN asked for.
        if app.screen == Screen::Reporter && configured_operator && !app.frozen {
            app.screen = Screen::Bracket;
        }

        // A screen opened via RSR_OPEN still needs its opened() hook (save
        // detection, manifest/hub reloads) — except under a seed, where
        // fixture state must not be overwritten by live lookups.
        let open_task = if app.frozen {
            Task::none()
        } else {
            match app.screen {
                Screen::Bracket => bracket::opened(&mut app),
                Screen::VodSplitter => vod_splitter::opened(&mut app),
                Screen::TagInstaller => tag_installer::opened(&mut app),
                Screen::Reporter => Task::none(),
            }
        };

        let mut boot = Task::batch([
            seed_task,
            open_task,
            if configured_operator && !app.frozen {
                current_sets::refresh(&mut app)
            } else {
                Task::none()
            },
        ]);
        // Screenshot hook: RSR_SCROLL=end snaps the main body to the bottom
        // so below-the-fold panels (Current Sets) can be captured.
        if std::env::var("RSR_SCROLL").as_deref() == Ok("end") {
            boot = Task::batch([
                boot,
                iced::advanced::widget::operate(
                    iced::advanced::widget::operation::scrollable::snap_to(
                        iced::advanced::widget::Id::new("main-body"),
                        iced::advanced::widget::operation::scrollable::RelativeOffset {
                            x: None,
                            y: Some(1.0),
                        },
                    ),
                ),
            ]);
        }
        (app, boot)
    }

    fn is_operator(&self) -> bool {
        self.st.config.configured && self.st.config.mode != "station"
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EngineState(v) => {
                let was_configured = self.st.config.configured;
                self.st = EngineState::from_value(&v);
                self.ready = true;
                if !was_configured && self.is_operator() {
                    // Finished onboarding straight into operator mode: pull
                    // the bracket immediately rather than waiting a cycle.
                    return current_sets::refresh(self);
                }
                Task::none()
            }
            Message::Tick => {
                self.now_s = station_core::now_sec();
                Task::none()
            }
            Message::SetsAutoRefresh => {
                if self.is_operator() && self.current_sets.busy.is_none() && !self.frozen {
                    current_sets::refresh(self)
                } else {
                    Task::none()
                }
            }
            Message::Onboarding(msg) => onboarding::update(self, msg),
            Message::Bracket(msg) => bracket::update(self, msg),
            Message::OpenBracket => {
                self.screen = Screen::Bracket;
                bracket::opened(self)
            }
            Message::Console(msg) => console::update(self, msg),
            Message::Sets(msg) => current_sets::update(self, msg),
            Message::Settings(msg) => settings::update(self, msg),
            Message::Vod(msg) => vod_splitter::update(self, msg),
            Message::OpenVodSplitter => {
                self.screen = Screen::VodSplitter;
                vod_splitter::opened(self)
            }
            Message::Tags(msg) => tag_installer::update(self, msg),
            Message::OpenTagInstaller => {
                self.screen = Screen::TagInstaller;
                tag_installer::opened(self)
            }
            Message::OpenSettings => {
                self.settings = Some(settings::State::new(&self.st.config, &self.engine));
                Task::none()
            }
            Message::ToggleLog => {
                self.show_log = !self.show_log;
                Task::none()
            }
            Message::CopyHubUrl => match &self.st.hub_url {
                Some(url) => iced::clipboard::write(url.clone()),
                None => Task::none(),
            },
            Message::CloseRequested(id) => {
                if self.tray.is_some() {
                    // Close-to-tray: the sender keeps running — a station
                    // must survive the TO absent-mindedly closing the window
                    // mid-bracket. The tray's Show brings it back.
                    iced::window::set_mode(id, iced::window::Mode::Hidden)
                } else {
                    // No tray host: hiding would strand the app (and a
                    // relaunch would double-post), so close means quit.
                    iced::exit()
                }
            }
            Message::TrayPoll => match self.tray.as_ref().and_then(|t| t.poll()) {
                Some(tray::TrayEvent::Show) => iced::window::latest().then(|id| match id {
                    Some(id) => Task::batch([
                        iced::window::set_mode(id, iced::window::Mode::Windowed),
                        iced::window::gain_focus(id),
                    ]),
                    None => Task::none(),
                }),
                Some(tray::TrayEvent::Quit) => iced::exit(),
                None => Task::none(),
            },
            Message::ScreenshotTick => {
                // On the splitter screen, wait for the preview thumbnails to
                // finish so the shot never shows empty frame placeholders —
                // with a ceiling so a stuck ffmpeg can't hang the capture.
                let vod_busy = self.screen == Screen::VodSplitter && !self.vod.thumbs_idle();
                // Likewise a live bracket read: capturing mid-fetch would
                // shoot the "Reading the bracket…" placeholder.
                let bracket_busy = self.screen == Screen::Bracket && self.bracket.loading;
                let waited = self.started.elapsed().as_secs();
                if (waited >= 2 && !vod_busy && !bracket_busy) || waited >= 20 {
                    return iced::window::latest()
                        .then(|id| match id {
                            Some(id) => iced::window::screenshot(id).map(Some),
                            None => Task::done(None),
                        })
                        .map(Message::Screenshotted);
                }
                Task::none()
            }
            Message::Screenshotted(shot) => {
                if let Some(shot) = shot {
                    let path =
                        std::env::var("RSR_SCREENSHOT").unwrap_or_else(|_| "shot.png".into());
                    let _ = image::save_buffer(
                        &path,
                        &shot.rgba,
                        shot.size.width,
                        shot.size.height,
                        image::ColorType::Rgba8,
                    );
                }
                std::process::exit(0);
            }
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run_with(self.feed.clone(), |feed: &Feed| {
                match feed.0.lock().unwrap().take() {
                    Some(rx) => rx.boxed(),
                    // Same id is already running; this instance is dropped.
                    None => iced::futures::stream::pending().boxed(),
                }
            })
            .map(Message::EngineState),
            iced::time::every(std::time::Duration::from_secs(10)).map(|_| Message::Tick),
            iced::window::close_requests().map(Message::CloseRequested),
        ];
        if self.tray.is_some() {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(300)).map(|_| Message::TrayPoll),
            );
        }
        if self.is_operator() {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(20))
                    .map(|_| Message::SetsAutoRefresh),
            );
        }
        if std::env::var_os("RSR_SCREENSHOT").is_some() {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(500))
                    .map(|_| Message::ScreenshotTick),
            );
        }
        Subscription::batch(subs)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = if !self.ready {
            container(iced::widget::text("starting…").color(theme::TEXT_MUTED))
                .center(Length::Fill)
                .into()
        } else if !self.st.config.configured {
            onboarding::view(self)
        } else if self.screen == Screen::Bracket {
            bracket::view(self)
        } else if self.screen == Screen::VodSplitter {
            vod_splitter::view(self)
        } else if self.screen == Screen::TagInstaller {
            tag_installer::view(self)
        } else {
            main_view::view(self)
        };

        let mut layers = stack![
            background::backdrop(),
            center(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(24),
        ];

        if self.show_log {
            layers = layers.push(log_overlay(self));
        }
        if let Some(s) = &self.settings {
            layers = layers.push(settings::view(self, s));
        }

        layers.into()
    }
}

/// The log drawer: a right-side monospace panel over the app.
fn log_overlay(app: &App) -> Element<'_, Message> {
    use iced::widget::{button, column, row, scrollable, text, Space};

    let mut lines = column![].spacing(2);
    for l in app.st.log.iter().rev() {
        lines = lines.push(
            text(l.clone())
                .font(theme::FONT_MONO)
                .size(11)
                .color(theme::TEXT_MUTED),
        );
    }

    let panel = container(
        column![
            row![
                text("Log")
                    .font(theme::FONT_DISPLAY)
                    .size(16)
                    .color(theme::TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("✕").size(14))
                    .style(theme::button_linkish)
                    .on_press(Message::ToggleLog),
            ]
            .align_y(iced::Alignment::Center),
            scrollable(lines).height(Length::Fill),
        ]
        .spacing(10),
    )
    .style(theme::drawer)
    .padding(18)
    .width(Length::Fixed(520.0))
    .height(Length::Fill);

    container(row![Space::new().width(Length::Fill), panel])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
