//! Tray + close-to-tray. The tray is best-effort per platform: when it comes
//! up (Windows and macOS), closing the window hides the app there and
//! reporting keeps running — the Tauri shell's contract. When there is no
//! tray, closing simply quits: a hidden window with no tray would strand the
//! app AND invite a second launch that double-posts sets.
//!
//! Linux is compiled out entirely (see Cargo.toml): tray-icon needs the GTK/
//! appindicator stack at build time and a gtk event loop at runtime, neither
//! of which this GTK-free app has. Linux `create()` returns None, engaging
//! the close-quits fallback — also the only sane behavior on a Deck in Game
//! Mode, where no tray host exists at all.

#[cfg(not(target_os = "linux"))]
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
#[cfg(not(target_os = "linux"))]
use tray_icon::{TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
}

#[cfg(target_os = "linux")]
pub struct Tray;

#[cfg(target_os = "linux")]
pub fn create() -> Option<Tray> {
    None
}

#[cfg(target_os = "linux")]
impl Tray {
    pub fn poll(&self) -> Option<TrayEvent> {
        None
    }
}

#[cfg(not(target_os = "linux"))]

pub struct Tray {
    // Held for its lifetime — dropping it removes the tray icon.
    _icon: TrayIcon,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
fn load_icon() -> Option<tray_icon::Icon> {
    let bytes: &[u8] = include_bytes!("../../assets/icons/128x128.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}
