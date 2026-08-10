//! Everything that shells out to ffmpeg/ffprobe.
//!
//! The desktop build uses whatever ffmpeg is on PATH (or next to the exe),
//! which is why it can stream-copy multi-GB VODs the browser build can't.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// Width of the preview thumbnails. Small on purpose — two per clip, and they
/// only need to be readable enough to confirm the cut landed in the right spot.
const THUMB_WIDTH: u32 = 240;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawn without flashing a console window on Windows — this is a GUI app.
fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// Prefer a binary sitting next to the exe (so a zip can ship one), then fall
/// back to PATH.
fn resolve(name: &str) -> String {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from));
    if let Some(dir) = exe_dir {
        let candidate = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        });
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

/// Is ffmpeg reachable at all? Checked once at startup so the UI can say so up
/// front rather than failing at split time.
pub async fn probe_available() -> bool {
    command(&resolve("ffmpeg"))
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// VOD length in seconds, via ffprobe. Used to clamp clip ends.
pub async fn duration(path: PathBuf) -> Option<f64> {
    let out = command(&resolve("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// One JPEG frame at `at` seconds, as bytes ready for `image::Handle`.
///
/// `-ss` goes before `-i` so ffmpeg seeks by keyframe instead of decoding the
/// whole file up to that point — the difference between instant and minutes on
/// a multi-hour VOD.
pub async fn thumbnail(path: PathBuf, at: f64) -> Option<Vec<u8>> {
    let out = command(&resolve("ffmpeg"))
        .args(["-v", "error", "-ss", &format!("{:.3}", at.max(0.0)), "-i"])
        .arg(&path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={THUMB_WIDTH}:-1"),
            "-f",
            "mjpeg",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if out.stdout.is_empty() {
        None
    } else {
        Some(out.stdout)
    }
}

/// Stream-copy one clip out of the VOD. No re-encode, so it's near-instant and
/// lossless; cuts land on the nearest keyframe, which is why the previews
/// matter.
pub async fn cut(
    vod: PathBuf,
    out_dir: PathBuf,
    filename: String,
    start: f64,
    duration: f64,
) -> Result<PathBuf, String> {
    if duration <= 0.0 {
        return Err("clip has no length".into());
    }
    let dest = out_dir.join(&filename);
    let status = command(&resolve("ffmpeg"))
        .args(["-v", "error", "-y", "-ss", &format!("{start:.3}"), "-i"])
        .arg(&vod)
        .args([
            "-t",
            &format!("{duration:.3}"),
            "-c",
            "copy",
            "-avoid_negative_ts",
            "make_zero",
        ])
        .arg(&dest)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "ffmpeg not found — install it and put it on your PATH.".to_string()
            } else {
                e.to_string()
            }
        })?;

    if status.status.success() {
        Ok(dest)
    } else {
        let err = String::from_utf8_lossy(&status.stderr);
        let last = err.lines().last().unwrap_or("ffmpeg failed").trim();
        Err(last.to_string())
    }
}

/// Default output folder: a `clips/` beside the VOD.
pub fn default_out_dir(vod: &Path) -> PathBuf {
    vod.parent()
        .map(|p| p.join("clips"))
        .unwrap_or_else(|| PathBuf::from("clips"))
}
