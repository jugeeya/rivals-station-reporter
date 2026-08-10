# Rivals Station Reporter

Desktop app for tournament stations running Rivals of Aether II: it watches
the game's stats save + replays, reconstructs each set as it's played, and
reports live scores to your bracket — same stack and look as the
[Rivals 2 Tag Tool](https://github.com/jugeeya/rivals-2-tag-tool)
(Tauri 2 + Vue 3, Rust core).

This is the Rust rewrite of the Python station reporter that lived in
[`jugeeya.github.io/matchlogger/sender/`](https://github.com/jugeeya/jugeeya.github.io/tree/main/matchlogger/sender)
— one installer instead of a Python install, a real UI instead of a Tk
widget. The full design is in `rivals-station-reporter-architecture.md`
(kept alongside this repo).

## What each PC runs

- **Station** — a PC people play on. Watches
  `Rivals2_StatsSaveSlot.sav` + the `Replays` folder (no game mod, no
  injection), rebuilds each game/set, and forwards them to the hub with this
  station's number.
- **Operator** — the TO machine. Runs the LAN hub every station posts to and
  is the only PC that talks to start.gg (the API token never leaves it).
  Gets the all-stations console: live scores stream to the bracket
  automatically, but **naming a winner is always an explicit click** —
  nothing ever advances the bracket on its own.
- **Both** — one PC doing both jobs.

There's also a built-in **Tag Installer** (header → Tag Installer): before
the bracket, pull every entrant's saved tag — name, colors, controls — from
the community tag database ([jugeeya.github.io/tags](https://jugeeya.github.io/tags/),
uploads happen in the [Rivals 2 Tag Tool](https://github.com/alex-mireles/rivals-2-tag-tool))
and install them into this setup's own Rivals 2 save. Paste the bracket URL
and Find: entrants are matched to published tags by their start.gg account
(never by name), matches are selected in one go, and whoever has nothing
published is listed so you know before they walk up. Installs check
save-format compatibility, overwrite same-named tags by default, never touch
slot 0 (the setup owner's tag), and rename to the start.gg tag when two
people share an in-game name so both land. Local `.r2tag` files install the
same way.

And a built-in **VOD Splitter** (header → VOD Splitter): point it at
a station's full OBS recording after the event and it cuts one clip per set,
named `[Tournament] P1 (Char) vs. P2 (Char) - Round`. It fetches the
bracket's set times from start.gg (no API token needed) and — because it
lives inside the reporter — automatically overlays this app's own
station-measured set times wherever a set was tracked, so those cuts land
where games actually started and ended instead of on whenever someone
clicked Start Match / Submit. Splitting on a different PC than the operator's?
Copy `hub-state.json` over (it's small) and pick it with "Choose file…".

The hub speaks the same `/matchlogger/*` HTTP API as the Cloudflare broker
(`jugeeya.github.io/broker/worker.js`), so stations can point at either, and
old Python stations interoperate with a Rust hub (and vice versa) during
migration. Online/ranked ladder games are detected via the save's game mode
and never touch the bracket.

## Screenshots

Sample data below, not a live event — the NATIVE app (Iced), captured by the
app itself: `./scripts/screenshots/native.sh` regenerates every image here
(see Development).

![First-run onboarding](docs/screenshots/onboarding.png)

First run: pick what this PC is; everything else is auto-detected or
validated inline.

![Operator console, three stations](docs/screenshots/operator-console.png)

The operator console watching three stations at once: live sets grouped
separately from finished-and-unreported ones, elapsed time and best-of taken
from start.gg's own data when present (station 1 reads 18m and "first to 4"
from the bracket, not the station's local guess), a per-game character strip
with each game's winner ringed, and the tag-to-entrant mapping the hub would
actually report, per player.

![Current Sets panel](docs/screenshots/available-sets.png)

The Current Sets panel, below the console: everything start.gg's bracket
shows happening right now, across the whole event. Playing now and startable
sets are grouped separately, each with its own station AND stream picker (a
set can sit at a station and on a stream at once); for a startable set,
picking and clicking Start Match happens as a single action.

![Station, idle between sets](docs/screenshots/station-idle.png)

A station between sets: health chips up top, waiting for the next game.

![Station, set in progress](docs/screenshots/station-live.png)

A station mid-set: live score, both tags, current characters, resolved
start.gg handles, and earlier sets below (online ladder games greyed out —
they never reach the bracket).

![Station, set just finished](docs/screenshots/station-finished.png)

The same station right after a set ends — station 3's LOOM/KIM set from the
operator screenshot, seen from its own PC. A station only ever knows "this
just finished"; awaiting-report status lives on the operator's hub.

![Settings drawer open](docs/screenshots/settings.png)

Settings: mode, event, hub/broker (with LAN auto-discovery), paths, and the
update checker, all editable without restarting.

![Tag Installer](docs/screenshots/tag-installer.png)

The Tag Installer, before the bracket: one Find against the event URL matched
four entrants' published tags (selected, marked "bracket") and named the two
entrants with nothing published. Install writes them into the game's own tag
save on this setup — names, colors, and controls, no in-game retyping.

![VOD Splitter](docs/screenshots/vod-splitter.png)

The VOD Splitter, after the event: station 3's recording cut into one clip
per set, with real preview frames at each edge and ±nudge buttons. Sets the
hub tracked carry a green ⏱ hub mark — their edges come from the station's
own measurements and rarely need touching — while the flagged row shows the
failure mode the warning exists for: start.gg never got a proper end time,
so the clip runs 57 minutes. Split in place with ffmpeg (lossless stream
copy), or export the cut list as CSV (LosslessCut), JSON, or a shell script.

## Install

Grab the build from the latest GitHub release: Windows portable `.zip`,
macOS `.app` zip, Linux/SteamOS `.AppImage`. The UI is native (Iced — pure
Rust, no webview), so there is no browser engine to configure on any
platform. First run walks through setup: pick what this PC is, paste the
start.gg event link (it echoes back the tournament name so a wrong paste is
caught immediately), enter the shared key from whoever runs the event —
done. Save/replay paths are auto-detected. Updates: Settings → "Check for
updates" downloads and applies the newest release in place.

On Windows and macOS, closing the window sends the app to the tray and
reporting keeps running ("Start with Windows" is in Settings). On Linux
there is no tray (that would drag the GTK stack back in); closing quits, so
leave the window open — or minimized — while sets are being played.

An existing `config.json` from the Python reporter uses the same keys and can
be pasted into Settings field-by-field (the file lives at the app config dir
once saved).

## VOD Splitter

Header → **VOD Splitter**: turn a station's full recording into one clip per
set, with filenames like

```
[The Hangout #1] jugeeya (Fleet) vs. Kimchi (Zetterburn) - Winners Quarter-Final.mp4
```

This works very well for single-stream setups and multi-recording setups
alike — it was built for [The Hangout](https://start.gg/thehangout), which
runs 4 recording setups to capture all winners-side and top 8 sets.

### What to do during your tournament

The idea is to automate VOD splitting by moving the VOD-marking process to
the ***start.gg match result submission time***. Sets this app's hub tracked
are covered automatically — their start/end times come from the game's own
save data, no discipline required. For everything else (untracked stations,
a stream setup without a reporter), the quality of the cut is the quality of
your start.gg timestamps:

1. **Use start.gg's `Start Match` exactly when you call the match, or as
   close to the true match start as possible.** start.gg stores that start
   time for the splitter to use later. Doing it when you call the match
   leaves some headroom while players get seated, and is much easier than
   watching for the exact game start.
2. **When a match ends, submit results only once you've confirmed:**
   - game count is correct
   - character data is correct
   - station number is correct

![Tournament workflow](docs/tournament-workflow.png)

The moment you hit `Submit Results`, start.gg takes that as the set's end
time. If any of the above gets edited *afterwards*, start.gg moves the end
time too — which is how a 10-minute set turns into the `unusually long`
warning. If that happens, you'll just set that clip's end time by hand.

### Recording types

- **OBS recordings** work out of the box. Use OBS's default naming scheme
  (it contains the timestamp), and the "Recording started" field fills
  itself from the filename.
- **Twitch VODs**: [Twitch VOD Downloader](https://chromewebstore.google.com/detail/twitch-vod-downloader/gaabmdjigfcnkgeommfpnoinpdmpfhaj?hl=en)
  gets you a `.chunked.ts` file (its UI also shows the VOD's start time),
  then `ffmpeg -i in.ts -c copy out.mp4` remuxes it into a file this
  program can work with.

### One-time setup: ffmpeg

Splitting in-app (and the preview frames) needs ffmpeg on your PATH. On
Windows, open Terminal and run `winget install ffmpeg`, then restart the
computer so PATH updates take effect.

<details><summary>If that doesn't work, click here for a manual installation method:</summary>

- Go to https://ffmpeg.org/download.html and download the Windows build.
- Extract it to a location like `C:\ffmpeg`.
- Press Win + X → "System" → "Advanced system settings" → "Environment
  Variables". Under "System variables", select "Path" → "Edit" → "New" and
  add the path to the ffmpeg `bin` folder.
- OK out of all dialogs and restart the computer (or your terminal).
- Verify: open a terminal and run `ffmpeg -version`.
</details>

### Splitting workflow

1. Open **VOD Splitter** from the header. The event is prefilled from this
   app's config (any start.gg event URL or slug works), and on the operator
   PC the hub's station-measured set times load automatically — the "Set
   times" row shows how many. Splitting on a different PC? Copy the operator's
   `hub-state.json` over (it's in the app config dir, e.g.
   `~/.config/io.github.jugeeya.rivals-station-reporter/` on Linux) and point
   "Choose file…" at it.
2. Click `Fetch sets`. The tournament display name fills itself if you left
   it blank.
3. Pick your station in the dropdown (e.g. Station 3).
4. `Choose VOD…` and pick the full OBS file.
5. "Recording started" pre-fills from the OBS filename — if it doesn't match
   the filename's hour:minutes:seconds, correct it to match.
6. Click `Build clips`. Hub-timed sets carry a green ⏱ mark; those edges
   basically never need touching. If a row is flagged
   **unusually long — check the end time**, find the true end in the VOD and
   type it in (Rivals 2 shows the current time in the top right, and the
   start time is reliable, so start from there). Lengths update as you type.
7. Click `Split with ffmpeg` and choose an output folder — or use
   `Export: CSV / JSON / ffmpeg script` to finish the job in another tool.
   The CSV imports straight into
   [LosslessCut](https://github.com/mifi/lossless-cut).

Cuts are stream copies (no re-encode), so they're fast and lossless, but they
land on the nearest keyframe rather than the exact millisecond. That's what
the preview frames are for — if a clip starts a beat early or late, nudge it
with the ±5s/30s/1m buttons.

There's also a browser version at
[jugeeya.github.io/vods](https://jugeeya.github.io/vods/) for quick jobs. It
runs ffmpeg via WebAssembly, which is fine for a few short clips but slow on
a multi-GB recording — that's what this built-in splitter is for. Prior art:
[CGuadagnino/startgg-vod-splitter](https://github.com/CGuadagnino/startgg-vod-splitter).

## Development

```sh
cargo run -p station-app     # the desktop app (native Iced UI)
cargo test -p station-core   # every ported behavior test (matching, stats,
                             # set machine, hub, forwarder, startgg client)
cargo test -p station-app    # engine-layer tests
./scripts/screenshots/native.sh   # regenerate the README screenshots
```

Dev tricks: `RSR_CONFIG_DIR=/tmp/profile` runs against a scratch profile
instead of the real one; `RSR_SCREENSHOT=out.png` renders the app, saves a
screenshot after ~2s, and exits; `RSR_SEED_STATE=seed.json` freezes fixture
data into the UI (`vodSplitter` / `tagInstaller` keys seed those screens);
`RSR_OPEN=settings|log|vod|tags` opens a drawer — or one of the tool
screens — on launch.

All logic lives in `crates/station-core` (no UI dependency) — 1:1 ports of
the Python modules with their tests. `crates/station-app` is the Iced shell:
an engine thread that owns the producer/forwarder/hub and pushes state
snapshots to the UI over one channel.

To check save-parsing parity against a real save:

```sh
RSR_REAL_SAVE="C:/Users/you/AppData/Local/Rivals2/Saved/SaveGames/Rivals2_StatsSaveSlot.sav" \
  cargo test -p station-core parses_real_save
```

## Release

Tag `v*` and push — CI builds the installers and attaches them to a draft
release. Unsigned binaries: Windows SmartScreen will warn on first run
("More info → Run anyway").
