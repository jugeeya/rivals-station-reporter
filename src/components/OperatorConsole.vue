<script setup lang="ts">
// Every station's sets in one console, with the three operator actions. Same
// semantics as the web console: Report opens a winner picker (suggested
// entrant emphasized) and is the ONLY thing that advances the bracket;
// Switch players flips who's who (characters + live score follow); Delete
// removes the record here without touching start.gg.
//
// Sets are split into three groups so a live set, a finished set waiting on
// a Report click, and an already-reported set never look interchangeable:
// see oc-group--live/actionable/other below. Within a group, sets sort by
// station number -- ingest time (the old sort) interleaved live/not-started/
// finished sets in a way that made the list hard to scan mid-bracket.
//
// Row rendering (per-game character strip, time column, mapping, best-of)
// lives in OperatorSetRow.vue / ../lib/operatorFormat.ts.

import { computed, ref } from 'vue';
import OperatorSetRow from './OperatorSetRow.vue';
import { state, reportWinner, swapPlayers, deleteSet } from '../lib/engine';
import { useNowSeconds } from '../lib/useNow';
import type { HubRecord } from '../types';

const pickerFor = ref<string | null>(null); // "station:setId"
const busy = ref(false);
const actionMsg = ref('');
const actionErr = ref(false);

// Drives the live rows' elapsed-time column (see useNow.ts).
const nowS = useNowSeconds();

const allSets = computed<HubRecord[]>(() => state.s.hubSnapshot.sets ?? []);
const sets = allSets; // used by the header's total count, any grouping included

function byStation(a: HubRecord, b: HubRecord): number {
  return Number(a.station) - Number(b.station);
}

// In progress, on the bracket: the section a TO checks first.
const liveSets = computed(() => allSets.value.filter((r) => r.status === 'live').sort(byStation));
// Finished, matched to a bracket set, not yet reported: the actionable list.
const actionableSets = computed(() =>
  allSets.value.filter((r) => r.status === 'matched').sort(byStation),
);
// Everything else -- already reported, or never reportable (online/ranked,
// match not started on start.gg, no bracket set at all) -- de-emphasized and
// collapsed by default so it doesn't compete with the two lists above.
const otherSets = computed(() =>
  allSets.value
    .filter((r) => r.status !== 'live' && r.status !== 'matched')
    .sort(byStation),
);

const stations = computed(() => {
  const st = state.s.hubSnapshot.stations ?? {};
  return Object.keys(st)
    .sort((a, b) => Number(a) - Number(b))
    .map((k) => ({ n: k, ...st[k] }));
});

function key(r: HubRecord): string {
  return `${r.station}:${r.id}`;
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
    'Players switched. Remembered for future sets.',
  );
}

async function onDelete(r: HubRecord) {
  const who = playersLabel(r) || r.id;
  // The dialog plugin, not window.confirm: WKWebView (the desktop app on
  // macOS) has no native confirm and silently returns false, which made
  // Delete a no-op there.
  const { confirm } = await import('@tauri-apps/plugin-dialog');
  const ok = await confirm(`Delete ${who} (station ${r.station})?\nstart.gg is untouched.`, {
    title: 'Delete set',
    kind: 'warning',
  });
  if (!ok) return;
  act(() => deleteSet(Number(r.station), String(r.id)), 'Set deleted.');
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function playersLabel(r: any): string {
  const players = r.set?.players ?? [];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return players.map((p: any) => p.name ?? '?').join(' vs ');
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

    <template v-else>
      <section v-if="liveSets.length" class="oc-group oc-group--live">
        <h3 class="oc-group-title">
          <span class="oc-group-dot" aria-hidden="true"></span>
          Live now
          <span class="oc-group-count">{{ liveSets.length }}</span>
        </h3>
        <ul class="oc-list">
          <OperatorSetRow
            v-for="r in liveSets"
            :key="key(r)"
            :record="r"
            :busy="busy"
            :active="pickerFor === key(r)"
            :now-s="nowS"
            @open-picker="pickerFor = key(r)"
            @close-picker="pickerFor = null"
            @pick="(id) => onReportPick(r, id)"
            @swap="onSwap(r)"
            @delete="onDelete(r)"
          />
        </ul>
      </section>

      <section v-if="actionableSets.length" class="oc-group oc-group--actionable">
        <h3 class="oc-group-title">
          Finished, awaiting report
          <span class="oc-group-count">{{ actionableSets.length }}</span>
        </h3>
        <ul class="oc-list">
          <OperatorSetRow
            v-for="r in actionableSets"
            :key="key(r)"
            :record="r"
            :busy="busy"
            :active="pickerFor === key(r)"
            :now-s="nowS"
            @open-picker="pickerFor = key(r)"
            @close-picker="pickerFor = null"
            @pick="(id) => onReportPick(r, id)"
            @swap="onSwap(r)"
            @delete="onDelete(r)"
          />
        </ul>
      </section>

      <details v-if="otherSets.length" class="oc-group oc-group--other">
        <summary class="oc-group-title">
          Reported / not actionable
          <span class="oc-group-count">{{ otherSets.length }}</span>
        </summary>
        <ul class="oc-list">
          <OperatorSetRow
            v-for="r in otherSets"
            :key="key(r)"
            :record="r"
            :busy="busy"
            :active="pickerFor === key(r)"
            :now-s="nowS"
            @open-picker="pickerFor = key(r)"
            @close-picker="pickerFor = null"
            @pick="(id) => onReportPick(r, id)"
            @swap="onSwap(r)"
            @delete="onDelete(r)"
          />
        </ul>
      </details>
    </template>
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

.oc-group {
  display: flex;
  flex-direction: column;
  gap: 0.2rem;
}

.oc-group-title {
  display: flex;
  align-items: center;
  gap: 0.4em;
  margin: 0;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-size: 0.7rem;
  font-weight: 700;
  color: var(--text-muted);
  cursor: default;

  // <summary> (the "other" group) gets the browser's default marker plus a
  // pointer cursor -- it's the one group meant to be opened/closed.
  .oc-group--other & { cursor: pointer; }
}

.oc-group-count {
  font-weight: 400;
  color: var(--text-muted);
}

// Live gets the loudest treatment (it's what a TO checks first); the pulse
// dot echoes LiveSetCard.vue's "now playing" indicator so the same visual
// language means "in progress" everywhere in the app.
.oc-group--live .oc-group-title { color: var(--accent); }

.oc-group-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent);
  animation: oc-group-pulse 2s ease-out infinite;
}

@keyframes oc-group-pulse {
  0% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 60%, transparent); }
  70% { box-shadow: 0 0 0 6px transparent; }
  100% { box-shadow: 0 0 0 0 transparent; }
}

.oc-group--actionable .oc-group-title { color: var(--text-warning); }

// Already reported / not reportable: collapsed by default (native <details>)
// so it never competes for attention with the two lists above.
.oc-group--other {
  summary::-webkit-details-marker { color: var(--text-muted); }
}

.oc-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 17rem;
  overflow-y: auto;
}
</style>
