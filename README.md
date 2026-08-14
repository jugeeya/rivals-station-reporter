# Rivals Station Reporter

Desktop app for tournament stations running Rivals of Aether II: it watches
the game's stats save + replays, reconstructs each set as it's played, and
reports live scores to your bracket. It also includes the [Tag Installer](#tag-installer) and [VOD Splitter](#vod-splitter)!

## What each PC runs

- **Station:** a PC people play on. Watches
  `Rivals2_StatsSaveSlot.sav` + the `Replays` folder, rebuilds each game/set, and forwards them to the operator hub with this
  station's number.
- **Operator:** the TO machine. Runs the LAN hub every station posts to and
  is the only PC that talks to start.gg.
  Gets the all-stations console: live scores stream to the bracket
  automatically, and a set that finishes **unambiguously** reports itself (see
  [Auto-report](#auto-report)). Anything it
  can't be sure of still waits for your click, and anything it got wrong is
  fixable after the fact with `edit result`.
- **Both:** one PC doing both jobs.

Online/ranked ladder games are detected via the save's game mode
and never touch the bracket.

## Setup

| [<img src="docs/screenshots/onboarding.png" width="430">](docs/screenshots/onboarding.png) | [<img src="docs/screenshots/settings.png" width="430">](docs/screenshots/settings.png) |
|:--:|:--:|
| **First run** | **Settings**, any time after |
| pick what this PC is | everything else, without restarting |

## Operator Screen

| [<img src="docs/screenshots/operator-console.png" width="430">](docs/screenshots/operator-console.png) | [<img src="docs/screenshots/available-sets.png" width="430">](docs/screenshots/available-sets.png) |
|:--:|:--:|
| **The console** | **Current Sets**, below it |
| every station's sets, live and awaiting | what start.gg says is happening now |

The console watches three stations at once: live sets grouped separately from
finished-and-unreported ones, elapsed time and best-of taken from start.gg's
own data when present, a per-game character/winner data series, and the tag-to-entrant mapping the hub would actually report,
per player.

Below it, Current Sets shows everything start.gg's bracket has happening
right now, across the whole event. Playing-now and startable sets are grouped
separately, each with its own station AND stream picker (a set can sit at a
station and on a stream at once); for a startable set, picking and clicking
Start Match happens as a single action.

## Bracket

| [<img src="docs/screenshots/bracket.png" width="880">](docs/screenshots/bracket.png) |
|:--:|
| **Mid-event** — every card state at once |

| [<img src="docs/screenshots/bracket-unstarted.png" width="430">](docs/screenshots/bracket-unstarted.png) | [<img src="docs/screenshots/bracket-done.png" width="430">](docs/screenshots/bracket-done.png) |
|:--:|:--:|
| **Not started on start.gg** | **Finished** |
| every set still a placeholder | the event as a record |

The event's whole tree, so nobody has to keep start.gg's page open on a
second monitor. Each set sits level with the sets that feed it, with the
lines drawn in, so it reads as a bracket rather than a grid of rounds.

Selecting a set opens its action bar — and if one of your stations tracked
that set, the station's own record appears under it: the tags it read, the
per-game characters, and the same Report / `edit result` / `switch players`
actions the Matches view has. Check who the station thinks is who before
anything advances, without leaving the tree.

| card | meaning |
|---|---|
| indigo, `● live` | being played right now |
| amber, `ready` | both seats filled, nobody has called it — the set to hand to the next free setup |
| `called` | assigned to a setup, not started yet |
| dimmed | finished; the winner's tag is green and their score bold |
| `—` seats | still waiting on an earlier round |

## Station Screen

| [<img src="docs/screenshots/station-live.png" width="880">](docs/screenshots/station-live.png) |
|:--:|
| **Mid-set** — live score as it's played |

| [<img src="docs/screenshots/station-idle.png" width="430">](docs/screenshots/station-idle.png) | [<img src="docs/screenshots/station-finished.png" width="430">](docs/screenshots/station-finished.png) |
|:--:|:--:|
| **Between sets** | **Just finished** |
| health chips, waiting for a game | the set it just closed out |

A station's whole job is on one screen: health chips up top, the set being
played front and centre, earlier sets below (online ladder games greyed out —
they never reach the bracket). Mid-set it shows the live score, both tags,
current characters and resolved start.gg handles.

The right-hand shot is station 3's LOOM/SLADE set from the operator
screenshot, seen from its own PC. A station only ever knows "this just
finished" — awaiting-report status, and the reporting itself, live on the
operator's hub.

## Auto-report

On by default (Settings, operator only). A set that finishes reports itself
on start.gg within a few seconds, instead of waiting for a Report click.
Wrong results can be fixed after the fact instead (see [below](#correcting-a-result)).

What still waits for you is anything the hub can't be sure about. A set
reports itself only when **all** of the following hold:

- it finished cleanly (the station's own journal says `complete`, so a set an
  idle timer closed out doesn't count),
- it is bound to a start.gg set (if nobody pressed Start Match on it, the
  report presses it for you — a TO who assigns setups but never calls them is
  normal, and refusing the report just stranded the set),
- it is a local bracket game, not an online or ranked match, and
- the winner is **certain**: their tag matched a bracket entrant exactly, or
  the loser's tag did (so the winner is the other entrant by elimination), or
  both players' partial matches independently agree on who is who. A lone
  partial match is good enough to pre-select for you; it is nowhere near good
  enough to advance a bracket unwatched.

### Correcting a result

The station reads results out of the game's own save data, which is right
almost always and wrong in ways it can't detect: a game the save never
recorded, a mis-read character, a set the idle timer cut short. `edit result`
on any row (including one that already reported) opens the games, one
line each with who won and what both players picked, plus add/remove game.
The score, the winner and the game count are all *derived* from those lines,
so they can't end up disagreeing with each other or with what start.gg is
told.

## Tag Installer

![Tag Installer](docs/screenshots/tag-installer.png)

Before the bracket, pull every entrant's saved tag from
the community tag database
([jugeeya.github.io/tags](https://jugeeya.github.io/tags/)) and install them into this setup's own Rivals 2 save. 

1. Paste the bracket URL and click `Find`
2. Entrants are matched to published tags by their start.gg account
3. You can view each tag's individual changes
4. Install those tags to this setup with `Install X tag(s)...`

## VOD Splitter

![VOD Splitter](docs/screenshots/vod-splitter.png)

Point it at a station's full OBS recording after the event and it cuts one clip per set,
named `[Tournament] P1 (Char) vs. P2 (Char) - Round`. It fetches the
bracket's set times from start.gg (no API token needed) and — because it
lives inside the reporter — automatically overlays the station's own
measured set times wherever a set was tracked, so those cuts land where
games actually started and ended instead of on whenever someone clicked
Start Match / Submit. The source is the per-set journals the station writes
as each set finishes (`<out dir>/sets/set_*.json` — written once, never
rewritten, so they survive restarts and later sessions); on the PC that both
played and recorded, they're found automatically. Splitting somewhere else?
Copy that station's `sets` folder over and pick it with "Choose folder…".

Filenames come out like

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
   app's config (any start.gg event URL or slug works), and this station's
   measured set times load automatically from its set journals — the "Set
   times" row shows how many. On the usual setup (the station PC records its
   own gameplay) that's everything, no clicking. Splitting a VOD recorded on
   a different PC? Copy that station's `sets` folder over (it's in the out
   dir: `matchlogger-out/sets/` under the app config dir, e.g.
   `~/.config/io.github.jugeeya.rivals-station-reporter/` on Linux, unless an
   out dir was configured) and point "Choose folder…" at it.
2. Click `Fetch sets`. The tournament display name fills itself if you left
   it blank.
3. Pick your station in the dropdown (e.g. Station 3).
4. `Choose VOD…` and pick the full OBS file.
5. "Recording started" pre-fills from the OBS filename — if it doesn't match
   the filename's hour:minutes:seconds, correct it to match.
6. Click `Build clips`. Station-timed sets carry a green ⏱ mark; those edges
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

## Install

Grab the build from the latest GitHub release: Windows portable `.zip`,
macOS `.app` zip, Linux/SteamOS `.AppImage`. The UI is native (Iced — pure
Rust, no webview), so there is no browser engine to configure on any
platform. First run walks through setup: pick what this PC is, paste the
start.gg event link (it echoes back the tournament name so a wrong paste is
caught immediately) — done. Stations find the operator's hub on the LAN by
themselves. Save/replay paths are auto-detected. Updates: Settings → "Check for
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
