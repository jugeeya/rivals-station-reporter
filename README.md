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

There's also a built-in **VOD Splitter** (header → VOD Splitter): point it at
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
data into the UI (a `vodSplitter` key seeds the splitter screen);
`RSR_OPEN=settings|log|vod` opens a drawer — or the VOD Splitter — on launch.

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
