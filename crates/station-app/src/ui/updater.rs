//! Native self-updater — the successor to the Tauri updater plugin.
//!
//! Checks GitHub's /releases/latest (which never points at the rolling
//! `beta` prerelease, the same property the old updater relied on), and
//! self-replaces per platform:
//!   * Linux/AppImage — download the new .AppImage next to the current one
//!     ($APPIMAGE, exported by the AppImage runtime) and rename it over.
//!   * Windows        — a running .exe can't be overwritten but CAN be
//!     renamed: current exe moves to .old, the new one is extracted from the
//!     release zip to the original path.
//!   * macOS          — same rename dance on the binary inside the .app.
//! Either way the switch takes effect on the next launch; "Restart now"
//! spawns the new binary and exits.
//!
//! Trust model: TLS to github.com — the same effective trust as downloading
//! from the Releases page by hand. The old updater additionally verified a
//! minisign signature; if that matters to you, the signing key and .sig
//! uploads can be layered onto this later.

use std::io::Read;
use std::path::PathBuf;

const REPO: &str = "jugeeya/rivals-station-reporter";

#[derive(Debug, Clone)]
pub struct Update {
    pub version: String,
    pub asset_url: String,
}

fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim_start_matches('v');
    // Prerelease/build suffixes never appear here (latest excludes them),
    // but be tolerant anyway.
    let core: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut it = core.split('.');
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ))
}

fn platform_asset_marker() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "x64-windows.zip"
    }
    #[cfg(target_os = "macos")]
    {
        "macos.zip"
    }
    #[cfg(target_os = "linux")]
    {
        "amd64.AppImage"
    }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // GitHub's API rejects requests with no User-Agent.
        .user_agent(concat!("rivals-station-reporter/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())
}

/// Blocking. `Ok(None)` = already up to date.
pub fn check() -> Result<Option<Update>, String> {
    let resp: serde_json::Value = client()?
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .send()
        .map_err(|e| format!("update check failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("update check failed: {e}"))?
        .json()
        .map_err(|e| format!("update check failed: {e}"))?;

    let tag = resp["tag_name"].as_str().unwrap_or_default().to_string();
    let (Some(remote), Some(local)) = (parse_semver(&tag), parse_semver(env!("CARGO_PKG_VERSION")))
    else {
        return Err(format!("could not parse versions ({tag})"));
    };
    if remote <= local {
        return Ok(None);
    }
    let marker = platform_asset_marker();
    let asset_url = resp["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|a| {
            let name = a["name"].as_str()?;
            name.ends_with(marker)
                .then(|| a["browser_download_url"].as_str().map(str::to_string))?
        })
        .ok_or_else(|| format!("release {tag} has no asset for this platform ({marker})"))?;
    Ok(Some(Update {
        version: tag.trim_start_matches('v').to_string(),
        asset_url,
    }))
}

/// Blocking. Downloads and stages the new binary; effective next launch.
pub fn apply(update: &Update) -> Result<(), String> {
    let mut bytes = Vec::new();
    client()?
        .get(&update.asset_url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download failed: {e}"))?
        .read_to_end(&mut bytes)
        .map_err(|e| format!("download failed: {e}"))?;

    #[cfg(target_os = "linux")]
    {
        // Running from an AppImage: replace the .AppImage file itself. A
        // plain binary run (dev) has nothing sane to replace — refuse.
        let target = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .ok_or("not running from an AppImage; download the new build manually")?;
        let staged = target.with_extension("new");
        std::fs::write(&staged, &bytes).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        std::fs::rename(&staged, &target).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Windows/macOS ship zips; the running binary is renamed aside (legal
        // on both platforms) and the new one lands at the original path.
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe_name = exe
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("could not resolve the executable name")?
            .to_string();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("bad archive: {e}"))?;
        let inner_name = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| n.ends_with(&exe_name))
            .ok_or("archive does not contain the app binary")?;
        let mut new_bin = Vec::new();
        archive
            .by_name(&inner_name)
            .map_err(|e| e.to_string())?
            .read_to_end(&mut new_bin)
            .map_err(|e| e.to_string())?;

        let old = exe.with_extension("old");
        let _ = std::fs::remove_file(&old); // leftover from a previous update
        std::fs::rename(&exe, &old).map_err(|e| format!("could not stage update: {e}"))?;
        if let Err(e) = std::fs::write(&exe, &new_bin) {
            // Roll back so the install isn't left headless.
            let _ = std::fs::rename(&old, &exe);
            return Err(format!("could not write update: {e}"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
        }
        Ok(())
    }
}

/// Spawn the (now updated) app and exit this instance.
pub fn restart() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let target = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or("no APPIMAGE path")?;
    #[cfg(not(target_os = "linux"))]
    let target = std::env::current_exe().map_err(|e| e.to_string())?;

    std::process::Command::new(target)
        .spawn()
        .map_err(|e| e.to_string())?;
    std::process::exit(0);
}
