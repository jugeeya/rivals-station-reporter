// Thin Tauri shell. All logic lives in the station-core crate; this side owns
// the engine thread, the tray, and the command surface.

mod commands;
mod config;
mod engine;
mod hub_glue;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's DMA-BUF renderer fails to initialize on a number of Linux
    // GPU/compositor stacks (SteamOS's gamescope, NVIDIA proprietary drivers,
    // some Wayland compositors) and then silently renders nothing — the app
    // opens as a blank white window. Falling back to the shared-memory
    // renderer costs some rendering speed, which this UI doesn't notice, and
    // works everywhere. Must be set before the first webview is created, and
    // only when the user hasn't already chosen a value themselves.
    #[cfg(target_os = "linux")]
    {
        // AppImage: the bundle ships its own libwebkit2gtk AND WebKit's
        // helper executables (WebKitWebProcess/WebKitNetworkProcess -- the
        // actual page lives in those), but libwebkit2gtk spawns them from a
        // path compiled in on the CI builder: /usr/lib/x86_64-linux-gnu/
        // webkit2gtk-4.1, Ubuntu's Debian-multiarch layout. On any distro
        // laid out differently -- SteamOS's Arch layout notably -- that path
        // doesn't exist, the web process never spawns, and the app is a
        // silent blank-white window (GTK is fine, so the window itself
        // opens; there's just no page and no error). The AppImage runtime
        // exports APPDIR; point WebKit at the bundled helpers explicitly.
        // Outside an AppImage (deb install, dev) APPDIR is unset and this
        // does nothing.
        if std::env::var_os("WEBKIT_EXEC_PATH").is_none() {
            if let Some(appdir) = std::env::var_os("APPDIR") {
                let exec = std::path::Path::new(&appdir)
                    .join("usr/lib/x86_64-linux-gnu/webkit2gtk-4.1");
                if exec.is_dir() {
                    let bundle = exec.join("injected-bundle");
                    std::env::set_var("WEBKIT_EXEC_PATH", &exec);
                    if bundle.is_dir()
                        && std::env::var_os("WEBKIT_INJECTED_BUNDLE_PATH").is_none()
                    {
                        std::env::set_var("WEBKIT_INJECTED_BUNDLE_PATH", bundle);
                    }
                }
            }
        }

        // AppImage again: WebKit 2.46+ launches WebKitWebProcess inside a
        // bubblewrap sandbox that bind-mounts a fixed set of system paths --
        // which does NOT include the AppImage's FUSE mount at /tmp/.mount_*.
        // The helper is therefore "inaccessible" from inside its own sandbox
        // and the launch SIGABRTs (confirmed via coredumpctl on a Steam
        // Deck), leaving a silent blank-white window. The sandbox cannot
        // work from an AppImage, so opt out of it there. The tradeoff is
        // real but small here: the webview only ever renders this app's own
        // bundled UI, never arbitrary web content. Installs that don't run
        // from an AppImage (deb, dev) keep the sandbox.
        if std::env::var_os("APPDIR").is_some()
            && std::env::var_os("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS").is_none()
        {
            std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
        }

        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        // On SteamOS the DMA-BUF fallback alone still comes up white for
        // many WebKit apps: accelerated compositing itself fails against
        // the Deck's gamescope/KDE stack, which is a separate knob. Turning
        // compositing off is WebKit's documented last-resort fix and costs
        // GPU-composited rendering (fine for this UI). Scoped to SteamOS
        // rather than all Linux so other distros keep the faster path;
        // detected via os-release (covers Desktop Mode) or the gamescope
        // session markers (covers Game Mode on non-SteamOS gamescope too).
        let on_steamos_or_gamescope = std::env::var_os("SteamDeck").is_some()
            || std::env::var("XDG_CURRENT_DESKTOP")
                .map(|v| v.eq_ignore_ascii_case("gamescope"))
                .unwrap_or(false)
            || std::fs::read_to_string("/etc/os-release")
                .map(|t| t.lines().any(|l| l.trim() == "ID=steamos"))
                .unwrap_or(false);
        if on_steamos_or_gamescope
            && std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none()
        {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // A second launch just fronts the existing window — two copies on
            // one PC would double-post every set.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // Updating N station PCs by hand mid-bracket is the thing this is
        // meant to avoid. `process` is what lets the app relaunch itself once
        // an update has been installed.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir().expect("app config dir");
            let engine = engine::start(app.handle().clone(), config_dir);
            app.manage(engine);

            // Tray: closing the window hides here and the sender keeps
            // running — a station must survive the TO absent-mindedly
            // closing the window mid-bracket.
            let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("window icon").clone())
                .menu(&menu)
                .tooltip("Rivals Station Reporter: reporting keeps running here")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::save_config,
            commands::resolve_event,
            commands::default_paths,
            commands::find_hubs,
            commands::report_winner,
            commands::swap_players,
            commands::delete_set,
            commands::list_available_sets,
            commands::start_match,
            commands::reassign_destination,
            commands::set_autostart,
            commands::get_autostart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
