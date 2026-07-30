<script setup lang="ts">
// Recent sets on THIS station, newest first (the live one is on its own card
// above). Online/ranked rows are greyed with the reason.

import { computed } from 'vue';
import type { Snapshot, SnapshotSet } from '../types';

const props = defineProps<{ snapshot: Snapshot }>();

const sets = computed<SnapshotSet[]>(() => [...props.snapshot.history].reverse());

function clock(epoch: number): string {
  const d = new Date(epoch * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

function reportable(s: SnapshotSet): boolean {
  return !s.mode || s.mode.toUpperCase() === 'LOCAL';
}

function badge(s: SnapshotSet): string {
  return reportable(s) ? '' : (s.mode ?? '').toLowerCase();
}

// Same public tag database hint as LiveSetCard — a submitted-tag -> handle
// lookup, not a bracket match. Deliberately styled weaker than the operator
// console's match cell; see .ss-sgg below.
function sggTitle(tag: string, sgg: string): string {
  return `Public tag database: "${tag}" is registered to start.gg @${sgg}. Not a bracket match, just what was submitted.`;
}
</script>

<template>
  <div class="ss">
    <div class="ss-head">
      <span class="ss-title">Sets today</span>
      <span v-if="sets.length" class="ss-count">{{ sets.length }}</span>
    </div>
    <p v-if="!sets.length" class="ss-empty">Finished sets will appear here.</p>
    <ul v-else class="ss-list">
      <li v-for="(s, i) in sets" :key="s.startEpoch + ':' + i" class="ss-row" :class="{ 'ss-row--grey': !reportable(s) }">
        <span class="ss-time">{{ clock(s.startEpoch) }}</span>
        <span class="ss-players">
          <template v-for="(p, j) in s.players" :key="p.tag + j">
            <span v-if="j > 0" class="ss-vs">vs</span>
            <span class="ss-tag" :class="{ 'ss-tag--won': p.won }">{{ p.tag }}</span>
            <span v-if="p.sgg" class="ss-sgg" :title="sggTitle(p.tag, p.sgg)">@{{ p.sgg }}</span>
            <span class="ss-char">({{ p.char }})</span>
          </template>
        </span>
        <span class="ss-score">{{ s.players.map((p) => p.wins).join('–') }}</span>
        <span v-if="badge(s)" class="ss-badge" title="Ladder game, never reported to the bracket">{{ badge(s) }}</span>
      </li>
    </ul>
  </div>
</template>

<style scoped lang="scss">
.ss {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}

.ss-head {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
}

.ss-title {
  text-transform: uppercase;
  letter-spacing: 0.1em;
  font-size: 0.75rem;
  color: var(--text-muted);
}

.ss-count { color: var(--text-muted); font-size: 0.75rem; opacity: 0.7; }

.ss-empty { margin: 0; font-size: 0.8rem; color: var(--text-muted); }

.ss-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 13rem;
  overflow-y: auto;
}

.ss-row {
  display: flex;
  align-items: baseline;
  gap: 0.6rem;
  padding: 0.35em 0.25em;
  border-bottom: 1px solid var(--line-divider);
  font-size: 0.85rem;

  &--grey { color: var(--text-muted); .ss-tag--won { color: inherit; } }
}

.ss-time {
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 0.75rem;
  color: var(--text-muted);
  flex: 0 0 auto;
}

.ss-players {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ss-vs { color: var(--text-muted); margin: 0 0.35em; font-size: 0.75em; }
.ss-tag { font-weight: 600; }
.ss-tag--won { color: var(--text-success); }

// Public tag database handle guess — weaker than the operator console's
// bracket match cell (.oc-match), so never the success/warning colors it
// uses. Just a muted hint riding along the same line, no extra height.
.ss-sgg { color: var(--text-muted); font-size: 0.8em; margin-left: 0.3em; }

.ss-char { color: var(--text-muted); font-size: 0.8em; margin-left: 0.25em; }

.ss-score {
  font-variant-numeric: tabular-nums;
  font-weight: 700;
  flex: 0 0 auto;
}

.ss-badge {
  flex: 0 0 auto;
  font-size: 0.7rem;
  padding: 0.1em 0.5em;
  border: 1px solid var(--line);
  border-radius: var(--radius-button);
  color: var(--text-warning);
}
</style>
