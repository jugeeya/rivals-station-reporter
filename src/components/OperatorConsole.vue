<script setup lang="ts">
// Every station's sets in one table, with the three operator actions. Same
// semantics as the web console: Report opens a winner picker (suggested
// entrant emphasized) and is the ONLY thing that advances the bracket;
// Switch players flips who's who (characters + live score follow); Delete
// removes the record here without touching start.gg.

import { computed, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import { state, reportWinner, swapPlayers, deleteSet } from '../lib/engine';
import type { HubRecord } from '../types';

const pickerFor = ref<string | null>(null); // "station:setId"
const busy = ref(false);
const actionMsg = ref('');
const actionErr = ref(false);

const sets = computed<HubRecord[]>(() => {
  const all = state.s.hubSnapshot.sets ?? [];
  return [...all].sort((a, b) => (b.ingestedAt ?? 0) - (a.ingestedAt ?? 0));
});

const stations = computed(() => {
  const st = state.s.hubSnapshot.stations ?? {};
  return Object.keys(st)
    .sort((a, b) => Number(a) - Number(b))
    .map((k) => ({ n: k, ...st[k] }));
});

function key(r: HubRecord): string {
  return `${r.station}:${r.id}`;
}

function clock(epoch?: number): string {
  if (!epoch) return '—';
  const d = new Date(epoch * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

function score(r: HubRecord): string {
  const players = r.set?.players ?? [];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return players.map((p: any) => p.wins ?? '?').join('–');
}

function playersLabel(r: HubRecord): string {
  const players = r.set?.players ?? [];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return players.map((p: any) => `${p.name ?? '?'} (${p.character ?? '?'})`).join(' vs ');
}

async function act(fn: () => Promise<unknown>, okMsg: string) {
  busy.value = true;
  actionMsg.value = '';
  try {
    await fn();
    actionMsg.value = okMsg;
    actionErr.value = false;
  } catch (e) {
    actionMsg.value = String(e);
    actionErr.value = true;
  } finally {
    busy.value = false;
    pickerFor.value = null;
  }
}

function onReportPick(r: HubRecord, entrantId: unknown) {
  act(() => reportWinner(Number(r.station), String(r.id), entrantId), 'Reported to start.gg.');
}

function onSwap(r: HubRecord) {
  act(
    () => swapPlayers(Number(r.station), String(r.id)),
    'Players switched — remembered for future sets.',
  );
}

function onDelete(r: HubRecord) {
  const who = playersLabel(r) || r.id;
  if (!window.confirm(`Delete ${who} (station ${r.station})?\nstart.gg is untouched.`)) return;
  act(() => deleteSet(Number(r.station), String(r.id)), 'Set deleted.');
}
</script>

<template>
  <div class="oc">
    <div v-if="stations.length" class="oc-stations">
      <div v-for="st in stations" :key="st.n" class="oc-station">
        <span class="oc-station-n">Stn {{ st.n }}</span>
        <span class="oc-station-state">{{ st.current?.state ?? 'idle' }}</span>
      </div>
    </div>

    <div class="oc-head">
      <span class="oc-title">All stations</span>
      <span v-if="sets.length" class="oc-count">{{ sets.length }} set{{ sets.length === 1 ? '' : 's' }}</span>
      <span v-if="actionMsg" class="oc-msg" :class="{ 'oc-msg--err': actionErr }">{{ actionMsg }}</span>
    </div>

    <p v-if="!sets.length" class="oc-empty">Sets from every station will appear here as they're played.</p>

    <ul v-else class="oc-list">
      <li v-for="r in sets" :key="key(r)" class="oc-row" :class="{ 'oc-row--grey': r.reportable === false }">
        <div class="oc-row-main">
          <span class="oc-cell oc-stn">{{ r.station }}</span>
          <span class="oc-cell oc-time">{{ clock(r.set?.endEpoch ?? r.ingestedAt) }}</span>
          <span class="oc-cell oc-players" :title="playersLabel(r)">{{ playersLabel(r) }}</span>
          <span class="oc-cell oc-score">{{ score(r) }}</span>
          <span class="oc-cell oc-round">
            {{ r.reportable === false ? (r.notReportableReason || 'not a bracket set') : (r.fullRoundText || '—') }}
          </span>
          <span class="oc-cell oc-status" :class="'oc-status--' + (r.status || 'recorded')">{{ r.status || 'recorded' }}</span>
        </div>
        <div class="oc-row-actions">
          <template v-if="pickerFor === key(r)">
            <span class="oc-pick-label">win:</span>
            <button
              v-for="e in r.entrants ?? []"
              :key="e.id"
              class="btn oc-btn"
              :class="{ 'oc-btn--suggested': String(e.id) === String(r.candidateWinnerEntrantId) }"
              :disabled="busy"
              :title="String(e.id) === String(r.candidateWinnerEntrantId) ? 'Suggested by name match — check before confirming' : ''"
              @click="onReportPick(r, e.id)"
            >
              <AppIcon v-if="String(e.id) === String(r.candidateWinnerEntrantId)" name="star" :size="13" />
              {{ e.name }}
            </button>
            <button class="icon-btn" title="Cancel" @click="pickerFor = null">
              <AppIcon name="close" :size="14" />
            </button>
          </template>
          <template v-else>
            <button
              class="btn oc-btn"
              :disabled="busy || r.reportable === false || !(r.entrants ?? []).length"
              :title="r.status === 'reported' ? 'Re-report with a different winner' : 'Report the winner to start.gg'"
              @click="pickerFor = key(r)"
            >
              {{ r.status === 'reported' ? 'Switch winner' : 'Report' }}
            </button>
            <button
              class="icon-btn"
              :disabled="busy || r.reportable === false"
              title="The station guessed who's who backwards — flip it (characters and live score follow)"
              @click="onSwap(r)"
            >
              <AppIcon name="swap" :size="15" />
            </button>
            <button
              class="icon-btn icon-btn--danger"
              :disabled="busy"
              title="Delete this set from the console (start.gg is untouched)"
              @click="onDelete(r)"
            >
              <AppIcon name="close" :size="14" />
            </button>
          </template>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped lang="scss">
.oc {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.oc-stations {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem;
}

.oc-station {
  display: inline-flex;
  gap: 0.4em;
  align-items: baseline;
  padding: 0.25em 0.6em;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-button);
  font-size: 0.75rem;
}

.oc-station-n { font-weight: 700; }
.oc-station-state { color: var(--text-muted); }

.oc-head {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
}

.oc-title {
  text-transform: uppercase;
  letter-spacing: 0.1em;
  font-size: 0.75rem;
  color: var(--text-muted);
}

.oc-count { color: var(--text-muted); font-size: 0.75rem; }
.oc-msg { margin-left: auto; font-size: 0.75rem; color: var(--text-success); }
.oc-msg--err { color: var(--text-failure); }

.oc-empty { margin: 0; font-size: 0.8rem; color: var(--text-muted); }

.oc-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 17rem;
  overflow-y: auto;
}

.oc-row {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  padding: 0.4em 0.25em;
  border-bottom: 1px solid var(--line-divider);
  font-size: 0.82rem;

  &--grey { color: var(--text-muted); }
}

.oc-row-main {
  display: flex;
  align-items: baseline;
  gap: 0.6rem;
}

.oc-cell { min-width: 0; }
.oc-stn { flex: 0 0 1.4rem; font-weight: 700; }
.oc-time { flex: 0 0 3rem; font-family: 'Ubuntu Sans Mono Variable', monospace; font-size: 0.75rem; color: var(--text-muted); }
.oc-players { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.oc-score { flex: 0 0 auto; font-weight: 700; font-variant-numeric: tabular-nums; }
.oc-round { flex: 0 0 9rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); }

.oc-status {
  flex: 0 0 5rem;
  text-transform: uppercase;
  font-size: 0.68rem;
  letter-spacing: 0.05em;

  &--reported { color: var(--text-success); }
  &--live { color: var(--accent); }
}

.oc-row-actions {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding-left: 2rem;
}

.oc-btn {
  display: inline-flex;
  align-items: center;
  gap: 0.3em;
  width: auto;
  padding: 0.3em 0.8em;
  font-size: 0.78rem;

  /* The name-matched candidate leads, but it's still a guess — emphasized,
     not pre-selected. */
  &--suggested { border-color: var(--accent); }
}

.oc-pick-label { font-size: 0.75rem; color: var(--text-muted); }
</style>
