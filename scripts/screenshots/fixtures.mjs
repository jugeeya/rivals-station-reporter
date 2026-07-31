// Fixture EngineState objects for the screenshot harness (see capture.mjs).
//
// Every hub-set record below mirrors the exact shape `record_for` builds in
// crates/station-core/src/hub.rs (~line 497), with `set` built by
// `summarize_set` / `derive_games` in crates/station-core/src/matching.rs.
// Nothing here is invented beyond that schema; where the operator scenario
// didn't specify a value (e.g. a losing player's character in a game that
// isn't otherwise described), a plausible one was picked and called out in
// the harness's own report, not silently guessed at in the schema itself.
//
// All timestamps hang off one fixed instant (FAKE_NOW_S) instead of
// Date.now(), so the fixtures -- and the screenshots built from them -- don't
// drift with wall-clock time. capture.mjs pins the browser's Date to this
// same instant via Playwright's clock API.

export const FAKE_NOW_S = Date.UTC(2027, 2, 6, 18, 42, 0) / 1000;
export const FAKE_NOW_MS = FAKE_NOW_S * 1000;

const HEALTH = {
  savePath: 'C:\\Users\\Josh\\AppData\\Local\\Rivals2\\Saved\\SaveGames\\Rivals2_StatsSaveSlot.sav',
  saveExists: true,
  saveArmed: true,
  replaysPath: 'C:\\Users\\Josh\\AppData\\Local\\Rivals2\\Saved\\Replays',
  replaysExists: true,
  outDir: 'C:\\Users\\Josh\\AppData\\Roaming\\rivals-station-reporter\\matchlogger-out',
};

const SLUG = 'tournament/rivals-weekly-42/event/singles';
const HUB_URL = 'http://192.168.1.42:8787';

/** Opaque station-generated set id -- an implementation detail, never
 *  rendered by the UI. Formatted like the app's own ids purely for realism. */
function toId(epochS) {
  const d = new Date(epochS * 1000);
  const pad = (n) => String(n).padStart(2, '0');
  return `${d.getUTCFullYear()}${pad(d.getUTCMonth() + 1)}${pad(d.getUTCDate())}_${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}${pad(d.getUTCSeconds())}`;
}

function baseConfig(overrides) {
  return {
    mode: 'station',
    station: 1,
    broker: HUB_URL,
    slug: SLUG,
    key: 'weekly42',
    startgg_token: '',
    save: '',
    replays: '',
    dir: '',
    idle: 420,
    poll: 2,
    hub_port: 8787,
    dry_run: false,
    configured: true,
    ...overrides,
  };
}

// ---- station-idle.png -------------------------------------------------------
// Station 1, configured, no live set: waiting between sets, with two finished
// sets already on the board (one reportable local set, one online ladder game
// that never touches the bracket -- the greyed/badged row).

export const stationIdle = {
  config: baseConfig({ station: 1 }),
  status: { msg: 'stats source armed | tags: jugeeya, KIM', error: false, t: FAKE_NOW_S },
  snapshot: {
    live: null,
    history: [
      {
        startEpoch: FAKE_NOW_S - 5400,
        complete: true,
        mode: null,
        games: 4,
        players: [
          { tag: 'jugeeya', char: 'Clairen', wins: 3, slot: 0, won: true, sgg: 'jugeeya' },
          { tag: 'KIM', char: 'Zetterburn', wins: 1, slot: 1, won: false },
        ],
      },
      {
        startEpoch: FAKE_NOW_S - 1800,
        complete: true,
        mode: 'ONLINE',
        games: 2,
        players: [
          { tag: 'jugeeya', char: 'Galvan', wins: 2, slot: 0, won: true, sgg: 'jugeeya' },
          { tag: 'RANDO', char: 'Fleet', wins: 0, slot: 1, won: false },
        ],
      },
    ],
  },
  hubSnapshot: { sets: [], stations: {} },
  hubUrl: null,
  log: [
    '18:06:00  stats source armed | tags: jugeeya, KIM',
    '18:15:00  finalized set_1804353120.json: jugeeya wins (4 games, complete=true)',
    '18:42:00  finalized set_1804356720.json: jugeeya wins (2 games, complete=true) [online, not reported]',
  ],
  health: HEALTH,
};

// ---- station-live.png -------------------------------------------------------
// Station 2, mid-set: same Brujita/jugeeya set the operator console shows for
// station 2, from this station's own point of view. First set of the day
// here, so no history yet.

export const stationLive = {
  config: baseConfig({ station: 2 }),
  status: { msg: 'tracking live: BRUJITA vs JUGZ!', error: false, t: FAKE_NOW_S },
  snapshot: {
    live: {
      startEpoch: FAKE_NOW_S - 360,
      complete: false,
      mode: null,
      games: 3,
      players: [
        { tag: 'BRUJITA', char: 'Maypul', wins: 2, slot: 0, won: true, sgg: 'brujita' },
        { tag: 'JUGZ!', char: 'Wrastor', wins: 1, slot: 1, won: false, sgg: 'jugeeya' },
      ],
    },
    history: [],
  },
  hubSnapshot: { sets: [], stations: {} },
  hubUrl: null,
  log: [
    '18:36:00  game 1 | BRUJITA(Maypul) vs JUGZ!(Clairen) -> BRUJITA wins',
    '18:38:00  game 2 | score 1-1',
    '18:41:00  game 3 | score 2-1',
  ],
  health: HEALTH,
};

// ---- station-finished.png ---------------------------------------------------
// Station 3, right after its set just ended: same LOOM/KIM set the operator
// console's "finished, awaiting report" row shows for station 3, from this
// station's own point of view. No hub configured here (matches idle/live:
// each station fixture is a lone station, the multi-station picture is
// operator-console.png), so there's nothing for this PC to know about
// "reportable" or "awaiting report" -- that status lives on the operator's
// hub record, not the station snapshot. The station only ever sees "here is
// what just finished."

export const stationFinished = {
  config: baseConfig({ station: 3 }),
  status: { msg: 'finalized set_1804355880.json: LOOM wins (4 games, complete=true)', error: false, t: FAKE_NOW_S },
  snapshot: {
    live: null,
    history: [
      {
        startEpoch: FAKE_NOW_S - 1320,
        complete: true,
        mode: null,
        games: 4,
        players: [
          { tag: 'LOOM', char: 'Zetterburn', wins: 3, slot: 0, won: true },
          { tag: 'KIM', char: 'Kragg', wins: 1, slot: 1, won: false },
        ],
      },
    ],
  },
  hubSnapshot: { sets: [], stations: {} },
  hubUrl: null,
  log: [
    '18:24:00  game 1 | LOOM(Ranno) vs KIM(Kragg) -> LOOM wins',
    '18:28:00  game 2 | score 1-1',
    '18:33:00  game 3 | score 2-1',
    '18:39:00  game 4 | score 3-1',
    '18:39:00  finalized set_1804355880.json: LOOM wins (4 games, complete=true)',
  ],
  health: HEALTH,
};

// ---- operator-console.png ---------------------------------------------------
// Three stations reporting to one operator's hub. Every set is best of 5
// (winsRequired: 3) with Start Match already pressed on start.gg
// (startggState: 2 == STARTGG_STATE_ONGOING, matching::STARTGG_STATE_ONGOING).
//
// Entrant ids are reused across records the way a real bracket would: E1
// jugeeya, E2 Kimchi, E3 Brujita, E4 Loom.
//
// candidateWinnerEntrantId/confidence are hand-computed the way
// matching::match_winner actually behaves, not invented: it only ever
// produces a candidate once a set is complete and `winnerName` is set (see
// matching.rs L184), so station 1 and station 2 (both still in progress) get
// "none" with no candidate -- station 3 (complete, winner "LOOM") normalizes
// to the same alias as entrant "Loom" ("loom" == "loom") for a "high"
// confidence match, exactly as norm()/entrant_names() would produce.
//
// Station 1 additionally carries startggStartedAt/startggTotalGames (see
// hub.rs's record_for) -- start.gg's own authoritative versions of the
// station's startEpoch/winsRequired guesses (operatorFormat.ts's
// timeText/bestOf prefer these when present). Deliberately given DIFFERENT
// values than the local guess they override (startggStartedAt 6 minutes
// earlier than set.startEpoch; startggTotalGames 7 where set.winsRequired
// implies a best-of-5) so the screenshot visibly demonstrates the override
// is actually being used, not merely that it doesn't crash: station 1 should
// read a longer elapsed time than its set.startEpoch alone would produce, and
// "first to 4" (ceil(7/2)) rather than "first to 3".

const station1Record = {
  id: toId(FAKE_NOW_S - 720),
  station: 1,
  ingestedAt: FAKE_NOW_S - 30,
  set: {
    setId: toId(FAKE_NOW_S - 720),
    complete: false,
    startEpoch: FAKE_NOW_S - 720,
    endEpoch: null,
    durationSeconds: null,
    winsRequired: 3,
    matchCount: 0,
    winnerSlot: null,
    winnerName: null,
    winnerCharacter: null,
    // No game has started yet, so no in-game tag/character data exists --
    // only the start.gg bind (entrants, below) is known this early.
    players: [
      { slot: 0, name: 'jugeeya', character: null, wins: 0 },
      { slot: 1, name: 'Kimchi', character: null, wins: 0 },
    ],
    games: [],
  },
  matchedStartggSetId: 'sgg-set-201',
  fullRoundText: 'Winners Quarter-Final',
  entrants: [
    { id: 'E1', name: 'jugeeya' },
    { id: 'E2', name: 'Kimchi' },
  ],
  slotEntrants: [
    { slot: 0, entrantId: 'E1', entrantName: 'jugeeya' },
    { slot: 1, entrantId: 'E2', entrantName: 'Kimchi' },
  ],
  candidateWinnerEntrantId: null,
  confidence: 'none',
  status: 'live',
  // No games yet -> handle_live never even pushes (gd is empty), so
  // "pending" is the honest state here, same as the real app.
  liveConfirmed: false,
  swap: false,
  mode: null,
  startggState: 2,
  // start.gg's authoritative startedAt (18 minutes ago, vs. the station's
  // own startEpoch guess of 12 minutes ago above) and totalGames (7, a
  // best-of-7, vs. the station's own winsRequired guess of 3 -- a
  // best-of-5) -- deliberately different from the local guesses so the
  // screenshot shows the override taking effect (18m elapsed, "first to 4"),
  // not just that these fields exist without crashing.
  startggStartedAt: FAKE_NOW_S - 1080,
  startggTotalGames: 7,
  reportable: true,
  notReportableReason: null,
};

const station2Record = {
  id: toId(FAKE_NOW_S - 360),
  station: 2,
  ingestedAt: FAKE_NOW_S - 20,
  set: {
    setId: toId(FAKE_NOW_S - 360),
    complete: false,
    startEpoch: FAKE_NOW_S - 360,
    endEpoch: null,
    durationSeconds: null,
    winsRequired: 3,
    matchCount: 3,
    winnerSlot: null,
    winnerName: null,
    winnerCharacter: null,
    players: [
      { slot: 0, name: 'BRUJITA', character: 'Maypul', wins: 2 },
      { slot: 1, name: 'JUGZ!', character: 'Wrastor', wins: 1 },
    ],
    games: [
      { gameNum: 1, winnerSlot: 0, chars: [{ slot: 0, character: 'Maypul' }, { slot: 1, character: 'Clairen' }] },
      { gameNum: 2, winnerSlot: 1, chars: [{ slot: 0, character: 'Maypul' }, { slot: 1, character: 'Clairen' }] },
      { gameNum: 3, winnerSlot: 0, chars: [{ slot: 0, character: 'Maypul' }, { slot: 1, character: 'Wrastor' }] },
    ],
  },
  matchedStartggSetId: 'sgg-set-202',
  fullRoundText: 'Winners Semi-Final',
  entrants: [
    { id: 'E3', name: 'Brujita' },
    { id: 'E1', name: 'jugeeya' },
  ],
  slotEntrants: [
    { slot: 0, entrantId: 'E3', entrantName: 'Brujita' },
    { slot: 1, entrantId: 'E1', entrantName: 'jugeeya' },
  ],
  candidateWinnerEntrantId: null,
  confidence: 'none',
  status: 'live',
  // Games recorded and pushed, and start.gg's read-back matched.
  liveConfirmed: true,
  swap: false,
  mode: null,
  startggState: 2,
  reportable: true,
  notReportableReason: null,
};

const station3Record = {
  id: toId(FAKE_NOW_S - 1320),
  station: 3,
  ingestedAt: FAKE_NOW_S - 180,
  set: {
    setId: toId(FAKE_NOW_S - 1320),
    complete: true,
    startEpoch: FAKE_NOW_S - 1320,
    endEpoch: FAKE_NOW_S - 190,
    durationSeconds: 1130,
    winsRequired: 3,
    matchCount: 4,
    winnerSlot: 0,
    winnerName: 'LOOM',
    winnerCharacter: 'Zetterburn',
    players: [
      { slot: 0, name: 'LOOM', character: 'Zetterburn', wins: 3 },
      { slot: 1, name: 'KIM', character: 'Kragg', wins: 1 },
    ],
    // Only the winner's per-game character was specified by the scenario
    // (G1/G3 Ranno, G4 Zetterburn); KIM's character wasn't called out for any
    // game it lost, so it's assumed steady on Kragg (its one specified pick,
    // from the game it won) throughout. Not rendered anywhere in the current
    // UI either way -- see the harness report.
    games: [
      { gameNum: 1, winnerSlot: 0, chars: [{ slot: 0, character: 'Ranno' }, { slot: 1, character: 'Kragg' }] },
      { gameNum: 2, winnerSlot: 1, chars: [{ slot: 0, character: 'Ranno' }, { slot: 1, character: 'Kragg' }] },
      { gameNum: 3, winnerSlot: 0, chars: [{ slot: 0, character: 'Ranno' }, { slot: 1, character: 'Kragg' }] },
      { gameNum: 4, winnerSlot: 0, chars: [{ slot: 0, character: 'Zetterburn' }, { slot: 1, character: 'Kragg' }] },
    ],
  },
  matchedStartggSetId: 'sgg-set-203',
  fullRoundText: 'Winners Round 3',
  entrants: [
    { id: 'E4', name: 'Loom' },
    { id: 'E2', name: 'Kimchi' },
  ],
  slotEntrants: [
    { slot: 0, entrantId: 'E4', entrantName: 'Loom' },
    { slot: 1, entrantId: 'E2', entrantName: 'Kimchi' },
  ],
  candidateWinnerEntrantId: 'E4',
  confidence: 'high',
  status: 'matched',
  swap: false,
  mode: null,
  startggState: 2,
  reportable: true,
  notReportableReason: null,
};

export const operatorConsole = {
  config: baseConfig({ mode: 'operator', station: 1, broker: '', startgg_token: 'sk_live_demo_token_123' }),
  status: {
    msg: `hub listening on 192.168.1.42:8787 | reporting to ${SLUG}`,
    error: false,
    t: FAKE_NOW_S,
  },
  snapshot: { history: [], live: null },
  hubSnapshot: {
    stations: {
      1: { current: { state: 'set_start' }, updatedAt: FAKE_NOW_S - 40 },
      2: { current: { state: 'set_open' }, updatedAt: FAKE_NOW_S - 15 },
      3: { current: { state: 'idle' }, updatedAt: FAKE_NOW_S - 200 },
    },
    sets: [station1Record, station2Record, station3Record],
  },
  hubUrl: HUB_URL,
  log: [
    '18:00:00  hub started on 192.168.1.42:8787',
    '18:30:00  station 1 bound to Winners Quarter-Final (jugeeya vs Kimchi)',
    '18:36:00  station 2 bound to Winners Semi-Final (Brujita vs jugeeya)',
    '18:39:10  station 3 set finalized: LOOM wins 3-1, not yet reported',
  ],
  health: HEALTH,
};

// ---- settings.png ------------------------------------------------------------
// Reuses the station-idle state as the backdrop; capture.mjs opens the
// Settings drawer on top of it via a real click, same as a user would.
export const settingsBase = stationIdle;

// ---- available-sets.png -------------------------------------------------------
// The Start Match panel, expanded: reuses the operator console backdrop
// (same three stations already reporting): one set start.gg shows as
// actually playing (state 2, with startggStartedAt/startggTotalGames so the
// panel's elapsed-time/best-of display has something real to show) and two
// startable sets -- one already assigned a station, one with none (exercises
// the inline station picker) -- mirroring the real shapes confirmed live
// against start.gg's API: a ready set has `entrant:{id,name}` on both slots.
// `browserMock.ts`'s own `availableSetsData` fixture is the browser
// dev-mode equivalent of this same shape. `availableSets` here is not part
// of EngineState in the real app either (it's `list_available_sets`, a
// separate query) -- capture.mjs's seeding passes it through to
// `window.__RSR_SEED_DATA__` alongside the engine state, and
// `browserMock.ts`'s `applySeed` peels it off into its own fixture object.
export const availableSets = {
  ...operatorConsole,
  availableSets: {
    sets: [
      {
        id: 'sgg-set-300',
        state: 2,
        fullRoundText: 'Winners Quarter-Final',
        station: 1,
        entrants: [
          { id: 'E1', name: 'jugeeya' },
          { id: 'E4', name: 'Kimchi' },
        ],
        startggStartedAt: FAKE_NOW_S - 12 * 60,
        startggTotalGames: 5,
      },
      {
        id: 'sgg-set-301',
        state: 1,
        fullRoundText: 'Winners Round 1',
        station: 2,
        entrants: [
          { id: 'E5', name: 'Loom' },
          { id: 'E6', name: 'Rando' },
        ],
      },
      {
        id: 'sgg-set-302',
        state: 1,
        fullRoundText: 'Losers Round 2',
        station: null,
        entrants: [
          { id: 'E3', name: 'Brujita' },
          { id: 'E2', name: 'Kimchi' },
        ],
      },
    ],
    stations: [{ number: 1 }, { number: 2 }, { number: 3 }],
  },
};
