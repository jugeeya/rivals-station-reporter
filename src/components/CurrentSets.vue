<script setup lang="ts">
// Current Sets: everything start.gg's bracket says is happening right now,
// across the whole event, in two groups.
//
// "Playing now" (state 2) is a DIFFERENT signal than OperatorConsole's own
// "LIVE NOW" section above it: that one only reflects what THIS app's own
// connected stations report, from local save-diffing. This one reflects
// start.gg's bracket directly, so it also covers a physical station not
// running this app at all, or any other set the bracket shows as started
// that this hub simply isn't tracking. The two can legitimately disagree.
//
// "Startable" is every not-yet-started set with both entrants determined (a
// set still waiting on a prior round -- e.g. an unseeded Grand Final --
// never appears here; the backend already filters those out).
//
// Occasional to check, not constant, so this still lives behind a <details>,
// visually secondary to the console's own live/actionable lists -- but open
// by default, since "what's happening on the bracket right now" is worth
// seeing without an extra click.
//
// Starting a match fires immediately on click, no confirmation dialog, same
// one-click standing as OperatorSetRow's Report button. A destination can be
// (re)assigned for either group: for a startable set, the pickers double as
// part of the Start Match click (assign then start, one user-facing action);
// for a playing-now set there's no "start" to also fire, so its pickers have
// their own small Change action that only ever reassigns.
//
// Station and stream are SEPARATE pickers, not one combined list: start.gg
// lets a set sit at a physical station AND on a stream at the same time
// (e.g. Station 1 + "socalrivals"), so an either/or dropdown couldn't
// express a real, common assignment. Picking the blank option on either
// leaves that half of the assignment untouched -- there is no unassign.

import { computed, onMounted, onUnmounted, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import DestinationDropdown, { type DropdownOption } from './DestinationDropdown.vue';
import { listAvailableSets, startMatch, reassignDestination } from '../lib/engine';
import { elapsedSince } from '../lib/operatorFormat';
import { useNowSeconds } from '../lib/useNow';
import type {
  AvailableSet,
  AvailableStation,
  AvailableStream,
  DestinationSelection,
} from '../types';

const STARTGG_STATE_ONGOING = 2;

const sets = ref<AvailableSet[]>([]);
const stations = ref<AvailableStation[]>([]);
const streams = ref<AvailableStream[]>([]);
const loaded = ref(false);
const loadErr = ref('');
const refreshing = ref(false);
const busyId = ref<string | null>(null);
const actionMsg = ref('');
const actionErr = ref(false);

// DestinationDropdown's v-model is string|null, so station numbers ride as
// strings ("3") and get parsed back in selection().
const stationOptions = computed<DropdownOption[]>(() => [
  { value: null, label: 'no station' },
  ...stations.value.map((st) => ({ value: String(st.number), label: `Station ${st.number}` })),
]);

const streamOptions = computed<DropdownOption[]>(() => [
  { value: null, label: 'no stream' },
  ...streams.value.map((st) => ({ value: st.name, label: st.name })),
]);

// One picked value per set per picker, keyed by set id -- seeded from the
// set's current station/stream (if any) so leaving the pickers untouched
// preserves today's assignment, and changed freely from there for either
// group. `seen*` remembers what the current assignment was at the last
// refresh: a pick still equal to it is untouched and follows start.gg (so a
// TO moving the set on the website doesn't leave a stale pick here that
// would light up Change and silently move it back), while a pick the
// operator changed survives refreshes until acted on.
const pickedStation = ref<Record<string, string | null>>({});
const pickedStream = ref<Record<string, string | null>>({});
const seenStation = ref<Record<string, string | null>>({});
const seenStream = ref<Record<string, string | null>>({});

// Ticks slowly so "elapsed" text stays honest between refreshes without
// every row needing its own timer (see useNow.ts).
const nowS = useNowSeconds();

function key(s: AvailableSet): string {
  return String(s.id);
}

function playersLabel(s: AvailableSet): string {
  return s.entrants.map((e) => e.name || '?').join(' vs ');
}

// Mirrors operatorFormat.ts's bestOf formula (best-of-N -> "first to
// ceil(N/2)"); not reused directly since bestOf takes a HubRecord and an
// available set is a different shape -- the actual math is one line.
function bestOfText(s: AvailableSet): string | null {
  return typeof s.startggTotalGames === 'number' && s.startggTotalGames > 0
    ? `first to ${Math.ceil(s.startggTotalGames / 2)}`
    : null;
}

const playingNow = computed(() => sets.value.filter((s) => s.state === STARTGG_STATE_ONGOING));
const startable = computed(() => sets.value.filter((s) => s.state !== STARTGG_STATE_ONGOING));

function currentStationKey(s: AvailableSet): string | null {
  return s.station != null ? String(s.station) : null;
}

function currentStreamKey(s: AvailableSet): string | null {
  return s.stream || null;
}

// What the pickers say should be sent for this set: null on either half
// means "leave that half as it is".
function selection(s: AvailableSet): DestinationSelection {
  const id = key(s);
  const st = pickedStation.value[id] ?? null;
  return { station: st != null ? Number(st) : null, stream: pickedStream.value[id] ?? null };
}

// Whether the pickers name anything different from the set's current
// assignment -- gates the Change button, since sending an unchanged
// selection would be a no-op.
function selectionChanged(s: AvailableSet): boolean {
  const id = key(s);
  const st = pickedStation.value[id] ?? null;
  const sm = pickedStream.value[id] ?? null;
  return (
    (st != null && st !== currentStationKey(s)) || (sm != null && sm !== currentStreamKey(s))
  );
}

// Guards against overlapping refreshes applying out of order: the 20s timer,
// the header button, and every post-action refresh can all be in flight at
// once, and a slow stale response landing last would resurrect a just-started
// set (complete with a live Start Match button). Only the newest request may
// write.
let refreshGen = 0;

async function refresh() {
  const gen = ++refreshGen;
  refreshing.value = true;
  try {
    const res = await listAvailableSets();
    if (gen !== refreshGen) return;
    loadErr.value = '';
    sets.value = res.sets ?? [];
    stations.value = res.stations ?? [];
    streams.value = res.streams ?? [];
    const liveIds = new Set(sets.value.map(key));
    for (const s of sets.value) {
      const id = key(s);
      const curSt = currentStationKey(s);
      const curSm = currentStreamKey(s);
      // Seed a new set's picks from its current assignment; re-sync a pick
      // the operator hasn't touched (still equal to the assignment we last
      // showed) so it tracks changes made elsewhere.
      if (!(id in pickedStation.value) || pickedStation.value[id] === seenStation.value[id]) {
        pickedStation.value[id] = curSt;
      }
      if (!(id in pickedStream.value) || pickedStream.value[id] === seenStream.value[id]) {
        pickedStream.value[id] = curSm;
      }
      seenStation.value[id] = curSt;
      seenStream.value[id] = curSm;
    }
    // Drop picks for sets that left the list (started elsewhere, completed,
    // bracket moved on) -- with a 20s auto-refresh these records would
    // otherwise grow for as long as the panel stays open.
    for (const id of Object.keys(pickedStation.value)) {
      if (!liveIds.has(id)) {
        delete pickedStation.value[id];
        delete pickedStream.value[id];
        delete seenStation.value[id];
        delete seenStream.value[id];
      }
    }
  } catch (e) {
    if (gen !== refreshGen) return;
    // Keep whatever was already listed rendering -- one transient start.gg
    // blip on the background refresh must not blank the whole panel (and any
    // picker the operator is mid-click on) for the next 20 seconds.
    loadErr.value = String(e);
  } finally {
    if (gen === refreshGen) {
      loaded.value = true;
      refreshing.value = false;
    }
  }
}

let refreshTimer: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  refresh();
  // Sets get called to stations / started / seeded from other rounds while
  // this panel sits open; a slow background refresh keeps the list honest
  // without the operator having to remember to click Refresh.
  refreshTimer = setInterval(refresh, 20_000);
});
onUnmounted(() => {
  if (refreshTimer) clearInterval(refreshTimer);
});

async function onStart(s: AvailableSet) {
  const id = key(s);
  busyId.value = id;
  actionMsg.value = '';
  try {
    await startMatch(String(s.id), selection(s));
    actionMsg.value = `Started ${playersLabel(s)}.`;
    actionErr.value = false;
    await refresh();
  } catch (e) {
    actionMsg.value = String(e);
    actionErr.value = true;
  } finally {
    busyId.value = null;
  }
}

// "station 3 and stream "socalrivals"" -- only the halves that actually
// changed, for the post-Change status line.
function changedLabel(s: AvailableSet, dest: DestinationSelection): string {
  const parts: string[] = [];
  if (dest.station != null && String(dest.station) !== currentStationKey(s)) {
    parts.push(`station ${dest.station}`);
  }
  if (dest.stream != null && dest.stream !== currentStreamKey(s)) {
    parts.push(`stream "${dest.stream}"`);
  }
  return parts.join(' and ');
}

async function onChangeDestination(s: AvailableSet) {
  const id = key(s);
  if (!selectionChanged(s)) return;
  const dest = selection(s);
  const label = changedLabel(s, dest);
  busyId.value = id;
  actionMsg.value = '';
  try {
    await reassignDestination(String(s.id), dest);
    actionMsg.value = `Moved ${playersLabel(s)} to ${label}.`;
    actionErr.value = false;
    await refresh();
  } catch (e) {
    actionMsg.value = String(e);
    actionErr.value = true;
  } finally {
    busyId.value = null;
  }
}
</script>

<template>
  <details class="as" open>
    <summary class="as-summary">
      Current Sets
      <span v-if="sets.length" class="as-count">{{ sets.length }}</span>
      <button
        class="icon-btn as-refresh"
        :class="{ 'as-refresh--spinning': refreshing }"
        :disabled="busyId !== null || refreshing"
        title="Refresh"
        @click.stop.prevent="refresh"
      >
        <AppIcon name="refresh" :size="14" />
      </button>
    </summary>

    <div class="as-body">
      <div v-if="actionMsg" class="as-head">
        <span class="as-msg" :class="{ 'as-msg--err': actionErr }">{{ actionMsg }}</span>
      </div>

      <!-- A refresh error only takes over the panel when there is nothing to
           show; with data already listed it appears alongside, so a transient
           start.gg blip on the 20s background refresh doesn't blank the lists
           (and any picker mid-interaction) until the next successful pass. -->
      <p v-if="loadErr && !sets.length" class="as-empty as-empty--err">{{ loadErr }}</p>
      <p v-else-if="loaded && !sets.length" class="as-empty">
        Nothing happening on the bracket right now.
      </p>

      <template v-else>
        <p v-if="loadErr" class="as-empty as-empty--err">refresh failed: {{ loadErr }}</p>
        <div v-if="playingNow.length" class="as-group">
          <div class="as-group-head as-group-head--live">
            <span class="as-group-dot" aria-hidden="true"></span>PLAYING NOW
            <span class="as-count">{{ playingNow.length }}</span>
          </div>
          <ul class="as-list">
            <li v-for="s in playingNow" :key="key(s)" class="as-row">
              <span class="as-round" :title="s.fullRoundText">{{ s.fullRoundText || '·' }}</span>
              <span class="as-players" :title="playersLabel(s)">{{ playersLabel(s) }}</span>
              <span v-if="s.startggStartedAt" class="as-elapsed">
                {{ elapsedSince(s.startggStartedAt, nowS) }}
              </span>
              <span v-if="bestOfText(s)" class="as-bestof">{{ bestOfText(s) }}</span>

              <DestinationDropdown
                v-model="pickedStation[key(s)]"
                class="as-picker"
                :options="stationOptions"
                :disabled="busyId === key(s)"
              />
              <DestinationDropdown
                v-if="streams.length"
                v-model="pickedStream[key(s)]"
                class="as-picker"
                :options="streamOptions"
                :disabled="busyId === key(s)"
              />
              <button
                class="linkish as-change"
                :disabled="busyId !== null || !selectionChanged(s)"
                title="Change this set's station and/or stream on start.gg (does not restart the match)"
                @click="onChangeDestination(s)"
              >
                Change
              </button>
            </li>
          </ul>
        </div>

        <div v-if="startable.length" class="as-group">
          <div class="as-group-head">
            STARTABLE
            <span class="as-count">{{ startable.length }}</span>
          </div>
          <ul class="as-list">
            <li v-for="s in startable" :key="key(s)" class="as-row">
              <span class="as-round" :title="s.fullRoundText">{{ s.fullRoundText || '·' }}</span>
              <span class="as-players" :title="playersLabel(s)">{{ playersLabel(s) }}</span>

              <DestinationDropdown
                v-model="pickedStation[key(s)]"
                class="as-picker"
                :options="stationOptions"
                :disabled="busyId === key(s)"
              />
              <DestinationDropdown
                v-if="streams.length"
                v-model="pickedStream[key(s)]"
                class="as-picker"
                :options="streamOptions"
                :disabled="busyId === key(s)"
              />

              <button
                class="btn as-btn"
                :disabled="busyId !== null"
                title="Start this match on start.gg now"
                @click="onStart(s)"
              >
                Start Match
              </button>
            </li>
          </ul>
        </div>
      </template>
    </div>
  </details>
</template>

<style scoped lang="scss">
.as {
  border-top: 1px solid var(--line-divider);
  padding-top: 0.4rem;
}

.as-summary {
  display: flex;
  align-items: center;
  gap: 0.4em;
  margin: 0;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-size: 0.7rem;
  font-weight: 700;
  color: var(--text-muted);
  cursor: pointer;

  &::-webkit-details-marker {
    color: var(--text-muted);
  }
}

// Sits at the end of the summary line rather than after it: `margin-left:
// auto` on a flex child pushes it to the far right without needing a
// separate wrapping element around "Current Sets" + the count.
.as-refresh {
  margin-left: auto;
}

.as-refresh--spinning svg {
  animation: as-spin 0.8s linear infinite;
}

@keyframes as-spin {
  to { transform: rotate(360deg); }
}

.as-count {
  font-weight: 400;
  color: var(--text-muted);
}

.as-body {
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
  padding-top: 0.4rem;
}

.as-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.as-msg {
  font-size: 0.75rem;
  color: var(--text-success);
}
.as-msg--err {
  color: var(--text-failure);
}

.as-empty {
  margin: 0;
  font-size: 0.8rem;
  color: var(--text-muted);
}
.as-empty--err {
  color: var(--text-failure);
}

.as-group {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.as-group-head {
  display: flex;
  align-items: center;
  gap: 0.4em;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  font-size: 0.65rem;
  font-weight: 700;
  color: var(--text-muted);
}

// Same accent the console's own LIVE NOW header uses (.oc-group--live's
// title color), so "playing now" reads as the same kind of fact wherever it
// shows up in this app.
.as-group-head--live {
  color: var(--accent);
}

.as-group-dot {
  width: 0.4em;
  height: 0.4em;
  border-radius: 50%;
  background: currentColor;
}

.as-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 14rem;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}

.as-row {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding: 0.35em 0.5em;
  border-bottom: 1px solid var(--line-divider);
  font-size: 0.8rem;
}

.as-round {
  flex: 0 0 8rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-muted);
}

.as-players {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.as-elapsed {
  flex: 0 0 auto;
  font-variant-numeric: tabular-nums;
  color: var(--accent);
  font-size: 0.75rem;
}

.as-bestof {
  flex: 0 0 auto;
  font-size: 0.65rem;
  color: var(--text-muted);
  white-space: nowrap;
}

// DestinationDropdown.vue owns its own closed/open styling now; this row only needs to
// say how the whole control sizes within the flex row.
.as-picker {
  flex: 0 0 auto;
}

.as-change {
  flex: 0 0 auto;
  font-size: 0.72rem;
}

.as-btn {
  flex: 0 0 auto;
  width: auto;
  padding: 0.3em 0.8em;
  font-size: 0.78rem;
}
</style>
