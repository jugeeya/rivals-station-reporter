// Shapes mirror the engine's `state()` JSON — dynamic where the wire is
// dynamic (records/sets are the same JSON the hub/broker speak).

export interface Config {
  mode: 'station' | 'operator' | 'both';
  station: number;
  broker: string;
  slug: string;
  key: string;
  startgg_token: string;
  save: string;
  replays: string;
  dir: string;
  idle: number;
  poll: number;
  hub_port: number;
  dry_run: boolean;
  configured: boolean;
}

export interface SnapshotPlayer {
  tag: string;
  char: string;
  wins: number;
  slot: number;
  won: boolean;
  /** The public tag database's resolved start.gg handle for this tag, or
   * null/absent when it doesn't recognize it. Just that lookup — unrelated
   * to the hub's bracket-derived entrant matching. */
  sgg?: string | null;
}

export interface SnapshotSet {
  startEpoch: number;
  complete: boolean;
  mode: string | null;
  games: number;
  players: SnapshotPlayer[];
}

export interface Snapshot {
  history: SnapshotSet[];
  live: SnapshotSet | null;
}

/** A hub set record — same JSON the web console reads; kept loose. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type HubRecord = Record<string, any>;

export interface HubSnapshot {
  sets: HubRecord[];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  stations: Record<string, any>;
}

export interface Health {
  savePath: string;
  saveExists: boolean;
  saveArmed: boolean;
  replaysPath: string;
  replaysExists: boolean;
  outDir: string;
}

export interface EngineState {
  config: Config;
  status: { msg: string; error: boolean; t: number };
  snapshot: Snapshot;
  hubSnapshot: HubSnapshot;
  hubUrl: string | null;
  log: string[];
  health: Health;
}

export interface EventSummary {
  slug: string;
  name: string;
  tournament: string;
  entrants: number | null;
}

/** One entrant on an available-to-start set (see AvailableSet). */
export interface AvailableSetEntrant {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  id: any;
  name: string;
}

/** A start.gg set with both entrants determined, not yet started -- what
 *  the Start Match panel lists. Mirrors `Startgg::available_sets`'s
 *  normalized shape (crates/station-core/src/startgg.rs). */
export interface AvailableSet {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  id: any;
  fullRoundText: string;
  station: number | null;
  entrants: AvailableSetEntrant[];
}

/** An event station, as `available_sets` reports it: the plain number this
 *  app already uses (`cfg.station`), the opaque id is resolved server-side
 *  and never sent to the frontend. */
export interface AvailableStation {
  number: number;
}

export interface AvailableSetsResponse {
  sets: AvailableSet[];
  stations: AvailableStation[];
}
