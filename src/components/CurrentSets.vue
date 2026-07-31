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
// one-click standing as OperatorSetRow's Report button. A station can be
// (re)assigned for either group: for a startable set, the picker doubles as
// part of the Start Match click (assign then start, one user-facing action,
// two GraphQL calls underneath); for a playing-now set there's no "start" to
// also fire, so its picker has its own small Change action that only ever
// reassigns.

import { computed, onMounted, onUnmounted, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import DestinationDropdown, { type DropdownOption } from './DestinationDropdown.vue';
import { listAvailableSets, startMatch, reassignDestination } from '../lib/engine';
import { elapsedSince } from '../lib/operatorFormat';
import type { AvailableSet, AvailableStation, AvailableStream, Destination } from '../types';

const STARTGG_STATE_ONGOING = 2;

// A destination is encoded as one string key so a single ref per set (like
// the plain station number this replaced) still works as the picker's
// v-model: "station:3", "stream:socalrivals", or null for "no destination".
const STATION_PREFIX = 'station:';
const STREAM_PREFIX = 'stream:';

function destKey(kind: 'station' | 'stream', value: number | string): string {
  return kind === 'station' ? `${STATION_PREFIX}${value}` : `${STREAM_PREFIX}${value}`;
}

function parseDestKey(key: string | null): Destination | null {
  if (key == null) return null;
  if (key.startsWith(STATION_PREFIX)) {
    return { kind: 'station', number: Number(key.slice(STATION_PREFIX.length)) };
  }
  if (key.startsWith(STREAM_PREFIX)) {
    return { kind: 'stream', name: key.slice(STREAM_PREFIX.length) };
  }
  return null;
}

const sets = ref<AvailableSet[]>([]);
const stations = ref<AvailableStation[]>([]);
const streams = ref<AvailableStream[]>([]);
const loaded = ref(false);
const loadErr = ref('');
const refreshing = ref(false);
const busyId = ref<string | null>(null);
const actionMsg = ref('');
const actionErr = ref(false);

// Stations first (the common case), then streams, each grouped so the type
// reads at a glance instead of one flat list of ambiguous names/numbers.
const destinationOptions = computed<DropdownOption[]>(() => [
  { value: null, label: 'no destination' },
  ...stations.value.map((st) => ({ value: destKey('station', st.number), label: `Station ${st.number}` })),
  ...streams.value.map((st) => ({ value: destKey('stream', st.name), label: `Stream: ${st.name}` })),
]);

// One picked destination key per set, keyed by set id -- seeded from the
// set's current station/stream (if any) so leaving the picker untouched
// preserves today's assignment, and changed freely from there for either
// group.
const picked = ref<Record<string, string | null>>({});

// Ticks slowly so "elapsed" text stays honest between refreshes without
// every row needing its own timer -- same pattern as OperatorConsole's nowS.
const nowS = ref(Date.now() / 1000);
let tickTimer: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  tickTimer = setInterval(() => {
    nowS.value = Date.now() / 1000;
  }, 30_000);
});
onUnmounted(() => {
  if (tickTimer) clearInterval(tickTimer);
});

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

function currentDestKey(s: AvailableSet): string | null {
  if (s.station != null) return destKey('station', s.station);
  if (s.stream) return destKey('stream', s.stream);
  return null;
}

async function refresh() {
  refreshing.value = true;
  loadErr.value = '';
  try {
    const res = await listAvailableSets();
    sets.value = res.sets ?? [];
    stations.value = res.stations ?? [];
    streams.value = res.streams ?? [];
    for (const s of sets.value) {
      if (!(key(s) in picked.value)) picked.value[key(s)] = currentDestKey(s);
    }
  } catch (e) {
    loadErr.value = String(e);
  } finally {
    loaded.value = true;
    refreshing.value = false;
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
    const dest = parseDestKey(picked.value[id] ?? null);
    await startMatch(String(s.id), dest);
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

function destinationLabel(dest: Destination): string {
  return dest.kind === 'station' ? `station ${dest.number}` : `stream "${dest.name}"`;
}

async function onChangeDestination(s: AvailableSet) {
  const id = key(s);
  const dest = parseDestKey(picked.value[id] ?? null);
  if (dest == null) return;
  busyId.value = id;
  actionMsg.value = '';
  try {
    await reassignDestination(String(s.id), dest);
    actionMsg.value = `Moved ${playersLabel(s)} to ${destinationLabel(dest)}.`;
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

      <p v-if="loadErr" class="as-empty as-empty--err">{{ loadErr }}</p>
      <p v-else-if="loaded && !sets.length" class="as-empty">
        Nothing happening on the bracket right now.
      </p>

      <template v-else>
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
                v-model="picked[key(s)]"
                class="as-picker"
                :options="destinationOptions"
                :disabled="busyId === key(s)"
              />
              <button
                class="linkish as-change"
                :disabled="
                  busyId !== null || picked[key(s)] == null || picked[key(s)] === currentDestKey(s)
                "
                title="Change this set's station or stream on start.gg (does not restart the match)"
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
                v-model="picked[key(s)]"
                class="as-picker"
                :options="destinationOptions"
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
