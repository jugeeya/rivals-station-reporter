<script setup lang="ts">
// The Start Match panel: every not-yet-started set on the bracket with both
// entrants determined (a set still waiting on a prior round -- e.g. an
// unseeded Grand Final -- never appears here; the backend already filters
// those out). Occasional, not constant, so this lives collapsed by default
// behind a <details>, visually secondary to the live/actionable set lists in
// OperatorConsole.vue above it.
//
// Starting a match fires immediately on click -- no confirmation dialog,
// same one-click standing as OperatorSetRow's Report button. If a set has no
// station yet, an inline picker lets the operator assign one as part of the
// same click: the backend resolves the chosen number to start.gg's opaque
// station id and assigns it before starting, so this is still one
// user-facing action even though it's two GraphQL calls underneath.

import { onMounted, onUnmounted, ref } from 'vue';
import { listAvailableSets, startMatch } from '../lib/engine';
import type { AvailableSet, AvailableStation } from '../types';

const sets = ref<AvailableSet[]>([]);
const stations = ref<AvailableStation[]>([]);
const loaded = ref(false);
const loadErr = ref('');
const busyId = ref<string | null>(null);
const actionMsg = ref('');
const actionErr = ref(false);

// One picked station number per not-yet-assigned set, keyed by set id.
const picked = ref<Record<string, number | null>>({});

function key(s: AvailableSet): string {
  return String(s.id);
}

function playersLabel(s: AvailableSet): string {
  return s.entrants.map((e) => e.name || '?').join(' vs ');
}

async function refresh() {
  loadErr.value = '';
  try {
    const res = await listAvailableSets();
    sets.value = res.sets ?? [];
    stations.value = res.stations ?? [];
    for (const s of sets.value) {
      if (s.station == null && !(key(s) in picked.value)) picked.value[key(s)] = null;
    }
  } catch (e) {
    loadErr.value = String(e);
  } finally {
    loaded.value = true;
  }
}

let refreshTimer: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  refresh();
  // Sets get called to stations / seeded from other rounds while this panel
  // sits open; a slow background refresh keeps the list honest without the
  // operator having to remember to click Refresh.
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
    const stationNumber = s.station == null ? picked.value[id] : null;
    await startMatch(String(s.id), stationNumber);
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
</script>

<template>
  <details class="as">
    <summary class="as-summary">
      Start Match
      <span v-if="sets.length" class="as-count">{{ sets.length }}</span>
    </summary>

    <div class="as-body">
      <div class="as-head">
        <button class="linkish" :disabled="busyId !== null" @click="refresh">Refresh</button>
        <span v-if="actionMsg" class="as-msg" :class="{ 'as-msg--err': actionErr }">{{ actionMsg }}</span>
      </div>

      <p v-if="loadErr" class="as-empty as-empty--err">{{ loadErr }}</p>
      <p v-else-if="loaded && !sets.length" class="as-empty">
        Nothing available to start right now.
      </p>

      <ul v-else class="as-list">
        <li v-for="s in sets" :key="key(s)" class="as-row">
          <span class="as-round" :title="s.fullRoundText">{{ s.fullRoundText || '·' }}</span>
          <span class="as-players" :title="playersLabel(s)">{{ playersLabel(s) }}</span>

          <span v-if="s.station != null" class="as-station">Station {{ s.station }}</span>
          <select
            v-else
            class="as-picker"
            :disabled="busyId === key(s)"
            v-model="picked[key(s)]"
          >
            <option :value="null">no station</option>
            <option v-for="st in stations" :key="st.number" :value="st.number">
              Station {{ st.number }}
            </option>
          </select>

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

.as-count {
  font-weight: 400;
  color: var(--text-muted);
}

.as-body {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
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

.as-station {
  flex: 0 0 auto;
  color: var(--text-muted);
  font-size: 0.75rem;
}

.as-picker {
  flex: 0 0 auto;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-button);
  color: var(--text-primary);
  font: inherit;
  font-size: 0.75rem;
  padding: 0.2em 0.4em;
}

.as-btn {
  flex: 0 0 auto;
  width: auto;
  padding: 0.3em 0.8em;
  font-size: 0.78rem;
}
</style>
