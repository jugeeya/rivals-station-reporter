// Thin Tauri shell. All logic lives in the station-core crate; this side owns
// the engine thread, the tray, and the command surface.

mod commands;
mod config;
mod engine;
mod hub_glue;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

/// Report a webview-setup decision on stderr.
///
/// This exists because of the exact shape of the bug it was written for: the
/// symptom is a blank white window, and the app's own log panel lives INSIDE
/// the webview that failed to come up, so it structurally cannot report a
/// webview failure. stderr is the only channel left -- run the AppImage from
/// a terminal to read it. Several rounds of Linux fixes shipped before this
/// existed, with no way to tell which of them had even applied.
fn note(msg: &str) {
    eprintln!("[webview] {msg}");
}

/// Set `key` only when the environment hasn't already, reporting either way.
///
/// The "don't override" half is deliberate and load-bearing: it means every
/// knob below can be flipped from a shell without a rebuild, so this is
/// bisectable in place --
/// `WEBKIT_DISABLE_COMPOSITING_MODE=0 ./Rivals*.AppImage` and so on.
fn set_default(key: &str, value: &str, why: &str) {
    match std::env::var(key) {
        Ok(existing) => note(&format!("{key} already={existing} (kept; not overriding)")),
        Err(_) => {
            std::env::set_var(key, value);
            note(&format!("{key}={value} ({why})"));
        }
    }
}

/// Put the AppImage's own library directories at the FRONT of
/// `LD_LIBRARY_PATH`, so anything this process spawns can find the bundled
/// libraries.
///
/// This is the fix for the SteamOS white screen. `WebKitWebProcess` -- the
/// helper the page actually lives in -- is linked with `RUNPATH=$ORIGIN`,
/// meaning it only ever looks in its own directory,
/// `usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/`. But the
/// `libwebkit2gtk-4.1.so.0` it needs is bundled three levels up, in
/// `usr/lib/`. So it cannot resolve its own library from its rpath and
/// depends entirely on inheriting a correct `LD_LIBRARY_PATH`. Measured on
/// the Deck: `ldd` on that helper reports `libwebkit2gtk-4.1.so.0 => not
/// found`, and `pgrep WebKitWebProcess` finds nothing while the window is up.
///
/// That single fact explains the whole bug, including why it was so quiet:
/// the helper dies in the dynamic loader before it runs a single
/// instruction, and the loader's error goes to the child's stderr, which
/// WebKit discards. No web process means no renderer to configure and no
/// page to load -- which is why disabling DMA-BUF, disabling compositing,
/// forcing XWayland, fixing asset URLs and enabling devtools all changed
/// nothing. They were all downstream of a process that never existed.
///
/// linuxdeploy's AppRun does export an `LD_LIBRARY_PATH` that covers
/// `usr/lib`, but it demonstrably is not reaching the helper. Rather than
/// depend on that, prepend the bundle's lib dirs here in the parent, before
/// any webview exists: children inherit the environment at spawn time.
/// Prepending rather than defaulting matters -- the variable is normally
/// already set, so `set_default` would decline to touch it.
fn prepend_bundle_lib_path(appdir: &std::ffi::OsStr) {
    let base = std::path::Path::new(appdir);
    // usr/lib holds the bundled libwebkit2gtk/libgtk; the multiarch dir holds
    // the webkit helper tree. Both are only added when they actually exist,
    // so a differently-laid-out bundle degrades to a no-op rather than
    // pointing the loader at nothing.
    let mut parts: Vec<String> = ["usr/lib", "usr/lib/x86_64-linux-gnu"]
        .iter()
        .map(|d| base.join(d))
        .filter(|p| p.is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        note("no bundled lib dirs found -- LD_LIBRARY_PATH left alone");
        return;
    }
    match std::env::var("LD_LIBRARY_PATH") {
        Ok(existing) if !existing.is_empty() => {
            note(&format!("inherited LD_LIBRARY_PATH={existing}"));
            parts.push(existing);
        }
        _ => note("no LD_LIBRARY_PATH inherited"),
    }
    let joined = parts.join(":");
    std::env::set_var("LD_LIBRARY_PATH", &joined);
    note(&format!("LD_LIBRARY_PATH={joined}"));
}

/// Linux webview bring-up. See [`note`] for why it narrates itself.
///
/// Guarded with `cfg!` rather than `#[cfg]` so it stays compiled and
/// type-checked on every platform (the branch is a compile-time constant, so
/// other targets optimize the whole thing away). That matters here more than
/// usual: this code only ever *runs* on the one OS that can't be built from
/// the machine it's developed on, so `#[cfg]` would mean shipping it to CI
/// unchecked.
fn linux_webview_setup() {
    if !cfg!(target_os = "linux") {
        return;
    }
    note(&format!("app v{}", env!("CARGO_PKG_VERSION")));

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
    //
    // Every branch reports, including the ones that skip: a silent skip here
    // is indistinguishable from a fix that ran and didn't help, which is
    // precisely the ambiguity that made this take several attempts.
    let appdir = std::env::var_os("APPDIR");
    match &appdir {
        Some(d) => {
            note(&format!("APPDIR={}", d.to_string_lossy()));
            // Must happen before the webview is created: this is what lets
            // the spawned WebKitWebProcess resolve the bundled libwebkit2gtk.
            prepend_bundle_lib_path(d);
        }
        None => note("APPDIR unset -- not running from an AppImage"),
    }
    if std::env::var_os("WEBKIT_EXEC_PATH").is_some() {
        note("WEBKIT_EXEC_PATH already set (kept; not overriding)");
    } else if let Some(appdir) = &appdir {
        let exec = std::path::Path::new(appdir).join("usr/lib/x86_64-linux-gnu/webkit2gtk-4.1");
        if exec.is_dir() {
            let bundle = exec.join("injected-bundle");
            std::env::set_var("WEBKIT_EXEC_PATH", &exec);
            note(&format!("WEBKIT_EXEC_PATH={}", exec.display()));
            let helper = exec.join("WebKitWebProcess");
            note(&format!("  WebKitWebProcess present: {}", helper.is_file()));
            if bundle.is_dir() && std::env::var_os("WEBKIT_INJECTED_BUNDLE_PATH").is_none() {
                std::env::set_var("WEBKIT_INJECTED_BUNDLE_PATH", &bundle);
                note(&format!("WEBKIT_INJECTED_BUNDLE_PATH={}", bundle.display()));
            } else {
                note(&format!(
                    "  injected-bundle dir present: {}",
                    bundle.is_dir()
                ));
            }
        } else {
            note(&format!(
                "SKIPPED WEBKIT_EXEC_PATH -- {} is not a directory, so the bundled \
                 web-process helpers are NOT where this expects them",
                exec.display()
            ));
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
    if appdir.is_some() {
        set_default(
            "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS",
            "1",
            "AppImage: bwrap can't reach the FUSE mount",
        );
    } else {
        note("sandbox left on -- not an AppImage");
    }

    // WebKitGTK's DMA-BUF renderer fails to initialize on a number of Linux
    // GPU/compositor stacks (SteamOS's gamescope, NVIDIA proprietary drivers,
    // some Wayland compositors) and then silently renders nothing. Falling
    // back to the shared-memory renderer costs rendering speed this UI never
    // notices. Must be set before the first webview is created.
    set_default(
        "WEBKIT_DISABLE_DMABUF_RENDERER",
        "1",
        "DMA-BUF renderer often never paints on Linux",
    );

    // On SteamOS the DMA-BUF fallback alone still comes up white for
    // many WebKit apps: accelerated compositing itself fails against
    // the Deck's gamescope/KDE stack, which is a separate knob. Turning
    // compositing off is WebKit's documented last-resort fix and costs
    // GPU-composited rendering (fine for this UI). Scoped to SteamOS
    // rather than all Linux so other distros keep the faster path;
    // detected via os-release (covers Desktop Mode) or the gamescope
    // session markers (covers Game Mode on non-SteamOS gamescope too).
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let distro_id = os_release
        .lines()
        .find_map(|l| l.trim().strip_prefix("ID="))
        .unwrap_or("unknown");
    note(&format!(
        "distro ID={distro_id}, XDG_CURRENT_DESKTOP={desktop:?}, XDG_SESSION_TYPE={session:?}"
    ));

    let on_steamos_or_gamescope = std::env::var_os("SteamDeck").is_some()
        || desktop.eq_ignore_ascii_case("gamescope")
        || distro_id.trim() == "steamos";
    note(&format!(
        "detected SteamOS/gamescope: {on_steamos_or_gamescope}"
    ));
    if on_steamos_or_gamescope {
        set_default(
            "WEBKIT_DISABLE_COMPOSITING_MODE",
            "1",
            "SteamOS: accelerated compositing fails against gamescope/KDE",
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    linux_webview_setup();

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
