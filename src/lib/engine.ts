// The UI's single data source: a reactive mirror of the engine's state,
// fed by the `engine-state` event (desktop) or polling (browser mock).

import { reactive } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  AvailableSetsResponse,
  Config,
  DestinationSelection,
  EngineState,
  EventSummary,
} from '../types';

const empty: EngineState = {
  config: {
    mode: 'station',
    station: 1,
    broker: '',
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
  status: { msg: 'starting…', error: false, t: 0 },
  snapshot: { history: [], live: null },
  hubSnapshot: { sets: [], stations: {} },
  hubUrl: null,
  log: [],
  health: {
    savePath: '',
    saveExists: false,
    saveArmed: false,
    replaysPath: '',
    replaysExists: false,
    outDir: '',
  },
};

export const state = reactive<{ ready: boolean; s: EngineState }>({ ready: false, s: empty });

function apply(next: EngineState) {
  state.s = next;
  state.ready = true;
}

export async function init() {
  apply(await invoke<EngineState>('get_state'));
  const mocked = (window as unknown as Record<string, unknown>).__RSR_MOCK__;
  if (mocked) {
    // Browser mock: no event system — poll instead.
    setInterval(async () => apply(await invoke<EngineState>('get_state')), 1000);
  } else {
    await listen<EngineState>('engine-state', (ev) => apply(ev.payload));
  }
}

export async function saveConfig(cfg: Config) {
  apply(await invoke<EngineState>('save_config', { cfg }));
}

export function resolveEvent(url: string): Promise<EventSummary> {
  return invoke<EventSummary>('resolve_event', { url });
}

export function defaultPaths(): Promise<{
  save: string;
  saveExists: boolean;
  replays: string;
  replaysExists: boolean;
}> {
  return invoke('default_paths');
}

// Operator actions. The winner report is the one action that advances the
// bracket — always an explicit click, never automatic.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function reportWinner(station: number, setId: string, winnerEntrantId: any) {
  return invoke('report_winner', { station, setId, winnerEntrantId });
}

export function swapPlayers(station: number, setId: string) {
  return invoke('swap_players', { station, setId });
}

export function deleteSet(station: number, setId: string) {
  return invoke('delete_set', { station, setId });
}

// Current Sets: both entrants determined, either playing now (state 2) or
// startable (state 1/6), plus the event's stations and streams. Starting a
// startable set optionally (re)assigns a station and/or a stream first as
// part of the same click (a set can carry both at once); a playing-now set
// can only have its destination changed (reassignDestination), never started
// again. Same standing as reportWinner: markSetInProgress, assignStation,
// and assignStream are only ever reached through these explicit actions.
export function listAvailableSets(): Promise<AvailableSetsResponse> {
  return invoke('list_available_sets');
}

export function startMatch(setId: string, destination: DestinationSelection) {
  return invoke('start_match', {
    setId,
    stationNumber: destination.station,
    streamName: destination.stream,
  });
}

export function reassignDestination(setId: string, destination: DestinationSelection) {
  return invoke('reassign_destination', {
    setId,
    stationNumber: destination.station,
    streamName: destination.stream,
  });
}
