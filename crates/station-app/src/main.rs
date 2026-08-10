// Prevents an extra console window on Windows in release. Same directive the
// Tauri shell carried; it matters just as much here.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod background;
mod engine;
mod model;
mod theme;
mod ui;
mod vodsplit;

use ui::{App, Message};

fn main() -> iced::Result {
    let mut app = iced::application(new, update, view).title("Rivals Station Reporter");
    for bytes in theme::FONT_BYTES {
        app = app.font(bytes);
    }
    app.default_font(theme::FONT_BODY)
        .window(iced::window::Settings {
            size: iced::Size::new(980.0, 760.0),
            min_size: Some(iced::Size::new(760.0, 560.0)),
            // Close is handled by the app: hide-to-tray when a tray exists,
            // exit otherwise (see Message::CloseRequested).
            exit_on_close_request: false,
            ..Default::default()
        })
        .theme(app_theme)
        .subscription(subscription)
        .run()
}

fn new() -> (App, iced::Task<Message>) {
    App::new()
}

fn update(app: &mut App, message: Message) -> iced::Task<Message> {
    app.update(message)
}

fn view(app: &App) -> iced::Element<'_, Message> {
    app.view()
}

fn app_theme(_app: &App) -> iced::Theme {
    iced::Theme::Dark
}

fn subscription(app: &App) -> iced::Subscription<Message> {
    app.subscription()
}
