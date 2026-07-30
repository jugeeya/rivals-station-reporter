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

The hub speaks the same `/matchlogger/*` HTTP API as the Cloudflare broker
(`jugeeya.github.io/broker/worker.js`), so stations can point at either, and
old Python stations interoperate with a Rust hub (and vice versa) during
migration. Online/ranked ladder games are detected via the save's game mode
and never touch the bracket.

## Screenshots

Sample data below, not a live event: a station waiting between sets, a
station mid-set, the operator console watching three stations at once, and
Settings open. Regenerate these with `pnpm screenshots` (see Development).

![Station, idle between sets](docs/screenshots/station-idle.png)

A station between sets: health checks green, waiting for the next game, two
finished sets already logged (one local, one online ladder game that's
greyed out since it never reaches the bracket).

![Station, set in progress](docs/screenshots/station-live.png)

A station mid-set: live score, both tags, current characters, and the
online/ranked note when it applies.

![Operator console, three stations](docs/screenshots/operator-console.png)

The operator console watching three stations at once: live sets grouped
separately from finished-and-unreported ones, elapsed time against an
absolute clock, best-of shown next to the score, a per-game character strip
with the winner marked, and the tag-to-entrant mapping the hub would actually
report, per player.

![Settings drawer open](docs/screenshots/settings.png)

Settings: mode, event, hub/broker (with LAN auto-discovery), paths, and the
update checker, all editable without restarting.

## Install

Grab the installer from the latest GitHub release (Windows NSIS `.exe`;
macOS `.dmg`). First run walks through setup: pick what this PC is, paste the
start.gg event link (it echoes back the tournament name so a wrong paste is
caught immediately), enter the shared key from whoever runs the event — done.
Save/replay paths are auto-detected.

Closing the window sends the app to the tray; reporting keeps running.
"Start with Windows" is in Settings.

An existing `config.json` from the Python reporter uses the same keys and can
be pasted into Settings field-by-field (the file lives at the app config dir
once saved).

## Development

```sh
pnpm install
pnpm dev          # UI in a plain browser — a scripted fake tournament drives it
pnpm tauri dev    # the real desktop app
cargo test -p station-core   # every ported behavior test (matching, stats,
                             # set machine, hub, forwarder, startgg client)
```

All logic lives in `crates/station-core` (no Tauri dependency) — 1:1 ports of
the Python modules with their tests. `src-tauri` is a thin shell: an engine
thread that owns the producer/forwarder/hub and pushes state to the UI over
one event.

### Screenshots

`pnpm screenshots` boots the same browser-mock UI as `pnpm dev`, but seeds an
exact fixture state instead of letting the scripted fake tournament run, and
writes PNGs to `docs/screenshots/`. It starts the dev server itself if one
isn't already listening on :1420, and shuts down anything it started when
it's done.

```sh
pnpm screenshots:setup   # once: installs the Chromium build Playwright needs
pnpm screenshots
```

The fixtures live in `scripts/screenshots/fixtures.mjs`; the capture script
(viewport, frozen clock, disabled animations, PNG output) is
`scripts/screenshots/capture.mjs`. See `src/dev/browserMock.ts`'s
`window.__RSR_SEED__` / `__RSR_SEED_DATA__` for how a fixture reaches the UI
without the real Tauri backend.

To check save-parsing parity against a real save:

```sh
RSR_REAL_SAVE="C:/Users/you/AppData/Local/Rivals2/Saved/SaveGames/Rivals2_StatsSaveSlot.sav" \
  cargo test -p station-core parses_real_save
```

## Release

Tag `v*` and push — CI builds the installers and attaches them to a draft
release. Unsigned binaries: Windows SmartScreen will warn on first run
("More info → Run anyway").
