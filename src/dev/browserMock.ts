// Lets the UI run in a plain browser (`pnpm dev`) for layout work, without the
// desktop shell. Inert inside the real app: install() returns immediately when
// Tauri's internals are already present.
//
// It shims `window.__TAURI_INTERNALS__.invoke` with fixtures and runs a
// scripted fake tournament: once "configured", a set gains a game every few
// seconds, finalizes, and the next one starts — so every state the views
// render (idle, live, finished, online/ranked, operator console) is reachable
// without a game PC. The engine store polls get_state() in mock mode
// (window.__RSR_MOCK__), so no event plumbing is needed here.
//
// A screenshot harness needs the opposite of the fake tournament: an exact,
// unmoving state instead of a scripted timer. `window.__RSR_SEED__(state)`
// replaces the whole mock EngineState in one shot and permanently disables
// the ticker (see `seeded` below) so nothing changes state out from under a
// capture. It's also readable as `window.__RSR_SEED_DATA__` set *before* this
// module runs (e.g. via Playwright's `addInitScript`), so the very first
// `get_state()` the app makes already returns the seeded fixture -- no
// default-state flash before the real content appears. `pnpm dev` never sets
// that global, so the scripted tournament below is untouched.

/* eslint-disable @typescript-eslint/no-explicit-any */

const now = () => Math.floor(Date.now() / 1000);

const state: any = {
  config: {
    mode: 'station',
    station: 3,
    broker: 'https://r2tag-broker.jdsambasivam.workers.dev',
    slug: '',
    key: '',
    startgg_token: '',
    save: '',
    replays: '',
    dir: '',
    idle: 420,
    poll: 2,
    hub_port: 8787,
    dry_run: false,
    configured: false,
  },
  status: { msg: 'stats source armed | tags: JUGZ!, KIM', error: false, t: now() },
  snapshot: { history: [], live: null },
  hubSnapshot: { sets: [], stations: {} },
  hubUrl: null,
  log: [`${new Date().toTimeString().slice(0, 8)}  stats source armed | tags: JUGZ!, KIM`],
  health: {
    savePath: 'C:\\Users\\Josh\\AppData\\Local\\Rivals2\\Saved\\SaveGames\\Rivals2_StatsSaveSlot.sav',
    saveExists: true,
    saveArmed: true,
    replaysPath: 'C:\\Users\\Josh\\AppData\\Local\\Rivals2\\Saved\\Replays',
    replaysExists: true,
    outDir: 'C:\\Users\\Josh\\AppData\\Roaming\\rivals-station-reporter\\matchlogger-out',
  },
};

function log(msg: string) {
  state.log.push(`${new Date().toTimeString().slice(0, 8)}  ${msg}`);
  if (state.log.length > 200) state.log.shift();
  state.status = { msg, error: /fail|error|warning/i.test(msg), t: now() };
}

// ---- fake tournament -------------------------------------------------------

// `sgg` is set on some tags (JUGZ!, BRUJITA) and deliberately left off others
// (KIM, LOOM) so both the "resolved handle" and "not in the public DB" cases
// are reachable in the browser preview.
const MATCHUPS = [
  [{ tag: 'JUGZ!', char: 'Galvan', sgg: 'jugeeya' }, { tag: 'KIM', char: 'Zetterburn' }],
  [{ tag: 'BRUJITA', char: 'Maypul', sgg: 'brujita' }, { tag: 'JUGZ!', char: 'Clairen', sgg: 'jugeeya' }],
  [{ tag: 'LOOM', char: 'Ranno' }, { tag: 'KIM', char: 'Kragg' }],
];
let matchup = 0;
let ticker: number | null = null;
// Set by applySeed() (see below) and checked here so a seeded fixture can
// never start ticking, even if something later calls save_config with
// configured: true.
let seeded = false;

function startFakeTournament() {
  if (seeded || ticker != null) return;
  ticker = window.setInterval(() => {
    if (!state.config.configured || state.config.mode === 'operator') return;
    const live = state.snapshot.live;
    if (!live) {
      const [a, b] = MATCHUPS[matchup % MATCHUPS.length];
      matchup += 1;
      // every third set is an online ladder game, to show the gating UI
      const mode = matchup % 3 === 0 ? 'ONLINE' : 'LOCAL';
      state.snapshot.live = {
        startEpoch: now(),
        complete: false,
        mode,
        games: 1,
        players: [
          { tag: a.tag, char: a.char, wins: 1, slot: 0, won: true, sgg: (a as any).sgg ?? null },
          { tag: b.tag, char: b.char, wins: 0, slot: 1, won: false, sgg: (b as any).sgg ?? null },
        ],
      };
      log(`game 1 | ${a.tag}(${a.char}) vs ${b.tag}(${b.char}) -> ${a.tag} wins`);
      return;
    }
    // play the next game; someone reaches 3 and the set finalizes
    const winnerIdx = Math.random() < 0.55 ? 0 : 1;
    const p = live.players[winnerIdx];
    p.wins += 1;
    live.games += 1;
    live.players.forEach((pl: any, i: number) => (pl.won = i === winnerIdx));
    log(`game ${live.games} | score ${live.players[0].wins}-${live.players[1].wins}`);
    if (p.wins >= 3) {
      live.complete = true;
      state.snapshot.history.push(live);
      state.snapshot.live = null;
      log(`finalized set_${live.startEpoch}.json: ${p.tag} wins (${live.games} games, complete=true)`);
      if (state.config.slug && state.config.mode !== 'operator') log(`ingested set_${live.startEpoch}.json`);
    }
  }, 4000);
}

// ---- Current Sets (available sets) fixtures ----------------------------------
// A separate query path from hubSnapshot (mirrors the real
// Startgg::available_sets/list_available_sets): everything start.gg's own
// bracket shows right now, not just sets already ingested from a station.
// Kept outside `state` since the real command isn't part of engine-state
// either -- the UI fetches it on its own (see CurrentSets.vue). One set
// already playing (state 2, with startggStartedAt/startggTotalGames so the
// elapsed-time/best-of display has something real to show), one startable
// set with a station already assigned, one startable set with a stream
// already assigned, one startable set with neither (exercises the inline
// picker), three stations and two streams to pick from.

let availableSetsData: any = { sets: [], stations: [], streams: [] };

function seedAvailableSets() {
  availableSetsData = {
    sets: [
      {
        id: 'sgg-set-300',
        state: 2,
        fullRoundText: 'Winners Quarter-Final',
        station: 1,
        stream: null,
        entrants: [
          { id: 'E1', name: 'jugeeya' },
          { id: 'E4', name: 'Kimchi' },
        ],
        startggStartedAt: now() - 12 * 60,
        startggTotalGames: 5,
      },
      {
        id: 'sgg-set-301',
        state: 1,
        fullRoundText: 'Winners Round 1',
        station: 2,
        stream: null,
        entrants: [
          { id: 'E5', name: 'Loom' },
          { id: 'E6', name: 'Rando' },
        ],
      },
      {
        id: 'sgg-set-303',
        state: 1,
        fullRoundText: 'Winners Round 2',
        station: null,
        stream: 'socalrivals',
        entrants: [
          { id: 'E7', name: 'Nap' },
          { id: 'E8', name: 'Sopo' },
        ],
      },
      {
        id: 'sgg-set-302',
        state: 1,
        fullRoundText: 'Losers Round 2',
        station: null,
        stream: null,
        entrants: [
          { id: 'E3', name: 'Brujita' },
          { id: 'E2', name: 'Kimchi' },
        ],
      },
    ],
    stations: [{ number: 1 }, { number: 2 }, { number: 3 }],
    streams: [{ name: 'socalrivals' }, { name: 'main-stage' }],
  };
}

// ---- operator fixtures -------------------------------------------------------

function seedHub() {
  const t = now();
  state.hubUrl = `http://192.168.1.42:${state.config.hub_port}`;
  state.hubSnapshot = {
    stations: {
      1: { current: { state: 'set_open' }, updatedAt: t - 5 },
      2: { current: { state: 'idle' }, updatedAt: t - 190 },
      3: { current: { state: 'set_open' }, updatedAt: t - 12 },
    },
    sets: [
      {
        id: '20260727_130001', station: 1, ingestedAt: t - 1500, status: 'reported',
        reportable: true, fullRoundText: 'Winners Round 1',
        matchedStartggSetId: '111',
        entrants: [{ id: 'E1', name: 'jugeeya' }, { id: 'E2', name: 'Kimchi' }],
        candidateWinnerEntrantId: 'E1', confidence: 'high',
        set: { endEpoch: t - 1500, players: [
          { slot: 0, name: 'JUGZ!', character: 'Galvan', wins: 3 },
          { slot: 1, name: 'KIM', character: 'Zetterburn', wins: 1 },
        ] },
      },
      {
        id: '20260727_133001', station: 3, ingestedAt: t - 300, status: 'matched',
        reportable: true, fullRoundText: 'Winners Round 2',
        matchedStartggSetId: '112',
        entrants: [{ id: 'E3', name: 'Brujita' }, { id: 'E1', name: 'jugeeya' }],
        candidateWinnerEntrantId: 'E3', confidence: 'low',
        set: { endEpoch: t - 300, players: [
          { slot: 0, name: 'BRUJITA', character: 'Maypul', wins: 3 },
          { slot: 1, name: 'JUGZ!', character: 'Clairen', wins: 2 },
        ] },
      },
      {
        id: '20260727_134500', station: 2, ingestedAt: t - 60, status: 'recorded',
        reportable: false, notReportableReason: 'online ladder game',
        entrants: [],
        set: { endEpoch: t - 60, players: [
          { slot: 0, name: 'LOOM', character: 'Ranno', wins: 3 },
          { slot: 1, name: 'RANDO', character: 'Fleet', wins: 0 },
        ] },
      },
    ],
  };
}

// ---- deterministic seeding (screenshot harness) ----------------------------

/** Replace the whole mock EngineState and permanently stop the ticker. Safe
 *  to call more than once (e.g. re-seeding between captures in one page).
 *  An optional `availableSets` key on `next` seeds the separate Start Match
 *  query too (not part of EngineState in the real app either -- see
 *  `availableSetsData` above); absent, it resets to empty. */
function applySeed(next: unknown) {
  if (ticker != null) {
    window.clearInterval(ticker);
    ticker = null;
  }
  seeded = true;
  const clone: any =
    typeof structuredClone === 'function'
      ? structuredClone(next)
      : JSON.parse(JSON.stringify(next));
  const seededAvailableSets = clone.availableSets;
  delete clone.availableSets;
  for (const k of Object.keys(state)) delete state[k];
  Object.assign(state, clone);
  availableSetsData = seededAvailableSets ?? { sets: [], stations: [], streams: [] };
}

// ---- invoke fixtures ---------------------------------------------------------

const handlers: Record<string, (args: any) => any> = {
  get_state: () => JSON.parse(JSON.stringify(state)),
  save_config: (a) => {
    state.config = { ...state.config, ...a.cfg };
    log('settings saved');
    if (state.config.configured) {
      startFakeTournament();
      if (state.config.mode !== 'station') {
        seedHub();
        seedAvailableSets();
      } else {
        state.hubUrl = null;
        state.hubSnapshot = { sets: [], stations: {} };
        availableSetsData = { sets: [], stations: [], streams: [] };
      }
    }
    return JSON.parse(JSON.stringify(state));
  },
  resolve_event: (a) => {
    if (!/tournament\/.+\/event\/.+|start.gg/.test(a.url)) {
      throw 'Expected a start.gg event URL like start.gg/tournament/…/event/…';
    }
    return {
      slug: 'tournament/rivals-weekly-42/event/singles',
      name: 'Singles',
      tournament: 'Rivals Weekly #42',
      entrants: 24,
    };
  },
  default_paths: () => ({
    save: state.health.savePath,
    saveExists: true,
    replays: state.health.replaysPath,
    replaysExists: true,
  }),
  report_winner: (a) => {
    const rec = state.hubSnapshot.sets.find((r: any) => String(r.id) === String(a.setId));
    if (rec) {
      rec.status = 'reported';
      rec.reportedWinnerEntrantId = a.winnerEntrantId;
      log(`reported ${a.setId} to start.gg`);
    }
    return { ok: true };
  },
  swap_players: (a) => {
    const rec = state.hubSnapshot.sets.find((r: any) => String(r.id) === String(a.setId));
    if (rec?.set?.players) {
      rec.set.players.forEach((p: any) => (p.slot = p.slot === 0 ? 1 : 0));
      rec.set.players.reverse();
      log('switched players, remembered for future sets and re-pushed');
    }
    return { ok: true, repushed: true };
  },
  delete_set: (a) => {
    state.hubSnapshot.sets = state.hubSnapshot.sets.filter(
      (r: any) => String(r.id) !== String(a.setId),
    );
    log('set deleted');
    return { ok: true };
  },
  list_available_sets: () => JSON.parse(JSON.stringify(availableSetsData)),
  start_match: (a) => {
    const set = availableSetsData.sets.find((s: any) => String(s.id) === String(a.setId));
    if (!set) throw 'Set not found or not available to start.';
    // Mirrors do_start_match: each requested destination (a set can carry a
    // station AND a stream at once) that differs from the current one is
    // reassigned; one that already matches is skipped.
    if (a.stationNumber != null && set.station !== a.stationNumber) {
      set.station = a.stationNumber;
    }
    if (a.streamName != null && set.stream !== a.streamName) {
      set.stream = a.streamName;
    }
    // Starting the match means it's no longer "available to start" (it
    // moves into "playing now" for real; the mock just drops it since this
    // fixture data isn't meant to simulate a whole bracket).
    availableSetsData.sets = availableSetsData.sets.filter((s: any) => s !== set);
    log(`started match for set ${a.setId} on start.gg`);
    return {
      ok: true,
      setId: a.setId,
      stationAssigned: a.stationNumber ?? null,
      streamAssigned: a.streamName ?? null,
    };
  },
  reassign_destination: (a) => {
    const set = availableSetsData.sets.find((s: any) => String(s.id) === String(a.setId));
    if (!set) throw 'Set not found or not available to start.';
    if (a.stationNumber == null && a.streamName == null) throw 'No station or stream specified.';
    if (a.stationNumber != null) set.station = a.stationNumber;
    if (a.streamName != null) set.stream = a.streamName;
    log(`assigned set ${a.setId} on start.gg`);
    return {
      ok: true,
      setId: a.setId,
      stationAssigned: a.stationNumber ?? null,
      streamAssigned: a.streamName ?? null,
    };
  },
  get_autostart: () => false,
  set_autostart: () => null,
  // LAN hub discovery. A real sweep finds nothing in a browser, so this fakes
  // the result — set `__RSR_MOCK_HUBS__` on window to 0 / 1 / 2 to exercise
  // the not-found, auto-connect and pick-one states of SettingsDrawer.
  find_hubs: async () => {
    await new Promise((r) => setTimeout(r, 600)); // the real sweep takes ~1-2s
    const n = (window as any).__RSR_MOCK_HUBS__ ?? 1;
    const all = [
      { url: 'http://192.168.1.42:8787', slug: 'the-hangout-47', startgg: true },
      { url: 'http://192.168.1.99:8787', slug: null, startgg: false },
    ];
    return { hubs: all.slice(0, n) };
  },
  // plugin shims used by drawers in browser mode
  'plugin:dialog|open': () => null,
  // A real browser DOES have window.confirm (unlike WKWebView, which is why
  // the app uses the dialog plugin) -- so the mock can just use it.
  'plugin:dialog|confirm': (a) => window.confirm(String(a?.message ?? 'Confirm?')),
  'plugin:window|close': () => null,
  'plugin:app|version': () => '0.0.0-dev',
};

export function install() {
  const w = window as unknown as Record<string, unknown>;
  if (w.__TAURI_INTERNALS__) return; // real app — never interfere

  w.__RSR_MOCK__ = true;
  w.__RSR_SEED__ = applySeed;
  w.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { windowLabel: 'main', label: 'main' },
    },
    invoke: async (cmd: string, args: any) => {
      const h = handlers[cmd];
      if (!h) {
        console.warn(`[browserMock] unhandled invoke: ${cmd}`);
        return null;
      }
      return h(args ?? {});
    },
  };

  // A capture harness stashes the fixture here via `addInitScript` (run
  // before this module executes), so it can be applied synchronously right
  // now -- the very first `get_state()` the app makes already returns the
  // seeded fixture instead of a flash of default state.
  if (w.__RSR_SEED_DATA__) applySeed(w.__RSR_SEED_DATA__);
}
