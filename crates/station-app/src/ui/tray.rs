//! Tray + close-to-tray. The tray is best-effort per platform: when it comes
//! up (Windows and macOS reliably; Linux needs an appindicator host), closing
//! the window hides the app there and reporting keeps running — the Tauri
//! shell's contract. When the tray can't be created (headless-ish Linux,
//! missing appindicator), closing simply quits: a hidden window with no tray
//! would strand the app AND invite a second launch that double-posts sets.

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub struct Tray {
    // Held for its lifetime — dropping it removes the tray icon.
    _icon: TrayIcon,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
}

pub fn create() -> Option<Tray> {
    let icon = load_icon()?;
    let show = MenuItem::new("Show", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append(&show).ok()?;
    menu.append(&quit).ok()?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("Rivals Station Reporter: reporting keeps running here")
        .build()
        .ok()?;
    Some(Tray {
        _icon: tray,
        show_id: show.id().clone(),
        quit_id: quit.id().clone(),
    })
}

impl Tray {
    /// Non-blocking poll of the global menu-event channel.
    pub fn poll(&self) -> Option<TrayEvent> {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show_id {
                return Some(TrayEvent::Show);
            }
            if ev.id == self.quit_id {
                return Some(TrayEvent::Quit);
            }
        }
        None
    }
}

fn load_icon() -> Option<tray_icon::Icon> {
    let bytes: &[u8] = include_bytes!("../../../../src-tauri/icons/128x128.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}
