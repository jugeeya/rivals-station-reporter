<script setup lang="ts">
// One set's row in the operator console: the at-a-glance line, a per-game
// character strip so the set's actual character history is never implied by
// just the last game played, the start.gg tag<->entrant mapping (both
// players, not winner-only), and the existing Report/swap/delete actions.
//
// All layout; the actual field reads live in ../lib/operatorFormat.ts so this
// stays a template plus wiring.

import { computed } from 'vue';
import AppIcon from './AppIcon.vue';
import CharIcon from './CharIcon.vue';
import type { HubRecord } from '../types';
import {
  playersLabel,
  score,
  bestOf,
  timeText,
  timeTitle,
  matchTier,
  matchTitle,
  matchFallback,
  mappingRows,
  games,
} from '../lib/operatorFormat';

const props = defineProps<{
  record: HubRecord;
  busy: boolean;
  active: boolean;
  nowS: number;
}>();

defineEmits<{
  'open-picker': [];
  'close-picker': [];
  pick: [entrantId: unknown];
  swap: [];
  delete: [];
}>();

const gameList = computed(() => games(props.record));
const mapping = computed(() => mappingRows(props.record));
const statusText = computed(() => String(props.record.status || 'recorded'));

const rowClasses = computed(() => ({
  'oc-row--grey': props.record.reportable === false,
  'oc-row--live': props.record.status === 'live',
  'oc-row--actionable': props.record.status === 'matched',
  'oc-row--reported': props.record.status === 'reported',
}));
</script>

<template>
  <li class="oc-row" :class="rowClasses">
    <div class="oc-row-main">
      <span class="oc-cell oc-stn">{{ record.station }}</span>
      <span class="oc-cell oc-time" :title="timeTitle(record, nowS)">{{ timeText(record, nowS) }}</span>
      <span class="oc-cell oc-players" :title="playersLabel(record)">{{ playersLabel(record) }}</span>
      <span class="oc-cell oc-round">
        {{ record.reportable === false ? (record.notReportableReason || 'not a bracket set') : (record.fullRoundText || '·') }}
      </span>
      <span class="oc-cell oc-score">
        {{ score(record) }}
        <span v-if="bestOf(record)" class="oc-bestof">{{ bestOf(record) }}</span>
      </span>
      <span class="oc-cell oc-status" :class="'oc-status--' + statusText">
        <span class="oc-status-dot" aria-hidden="true"></span>{{ statusText }}
      </span>
    </div>

    <div class="oc-row-detail">
      <div v-if="gameList.length" class="oc-strip">
        <div v-for="g in gameList" :key="g.gameNum" class="oc-game" :title="'Game ' + g.gameNum">
          <span
            v-for="c in g.chars"
            :key="c.slot"
            class="oc-game-side"
            :class="{
              'oc-game-side--won': g.winnerSlot != null && g.winnerSlot === c.slot,
              'oc-game-side--lost': g.winnerSlot != null && g.winnerSlot !== c.slot,
            }"
          >
            <CharIcon :character="c.character" :size="18" />
          </span>
        </div>
      </div>
      <div v-else class="oc-strip oc-strip--empty">no games yet</div>

      <div class="oc-cell oc-match" :class="'oc-match--' + matchTier(record)" :title="matchTitle(record)">
        <template v-if="mapping">
          <span v-for="row in mapping" :key="row.tag" class="oc-match-row">
            <span class="oc-match-tag">{{ row.tag }}</span>
            <span class="oc-match-arrow" aria-hidden="true">&rarr;</span>
            <span class="oc-match-entrant">{{ row.entrant ?? '?' }}</span>
          </span>
        </template>
        <span v-else>{{ matchFallback(record) }}</span>
      </div>
    </div>

    <div class="oc-row-actions">
      <template v-if="active">
        <span class="oc-pick-label">win:</span>
        <button
          v-for="e in record.entrants ?? []"
          :key="e.id"
          class="btn oc-btn"
          :class="{ 'oc-btn--suggested': String(e.id) === String(record.candidateWinnerEntrantId) }"
          :disabled="busy"
          :title="String(e.id) === String(record.candidateWinnerEntrantId) ? 'Suggested by name match. Check before confirming' : ''"
          @click="$emit('pick', e.id)"
        >
          <AppIcon v-if="String(e.id) === String(record.candidateWinnerEntrantId)" name="star" :size="13" />
          {{ e.name }}
        </button>
        <button class="icon-btn" title="Cancel" @click="$emit('close-picker')">
          <AppIcon name="close" :size="14" />
        </button>
      </template>
      <template v-else>
        <button
          class="btn oc-btn"
          :disabled="busy || record.reportable === false || !(record.entrants ?? []).length"
          :title="record.status === 'reported' ? 'Re-report with a different winner' : 'Report the winner to start.gg'"
          @click="$emit('open-picker')"
        >
          {{ record.status === 'reported' ? 'Switch winner' : 'Report' }}
        </button>
        <button
          class="icon-btn"
          :disabled="busy || record.reportable === false"
          title="The station guessed who's who backwards. Flip it (characters and live score follow)"
          @click="$emit('swap')"
        >
          <AppIcon name="swap" :size="15" />
        </button>
        <button
          class="icon-btn icon-btn--danger"
          :disabled="busy"
          title="Delete this set from the console (start.gg is untouched)"
          @click="$emit('delete')"
        >
          <AppIcon name="close" :size="14" />
        </button>
      </template>
    </div>
  </li>
</template>

<style scoped lang="scss">
.oc-row {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.4em 0.5em;
  border-bottom: 1px solid var(--line-divider);
  border-left: 3px solid transparent;
  border-radius: 0.2em;
  font-size: 0.82rem;

  // Live and finished sets used to render identically apart from a small
  // grey status word; these three states now carry their own border/tint so
  // the section they're in is obvious even before reading any text.
  &--live {
    border-left-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 7%, transparent);
  }

  &--actionable {
    border-left-color: var(--text-warning);
    background: color-mix(in srgb, var(--text-warning) 6%, transparent);
  }

  &--reported {
    opacity: 0.68;
  }

  &--grey { color: var(--text-muted); }
}

.oc-row-main {
  display: flex;
  align-items: baseline;
  gap: 0.6rem;
}

.oc-cell { min-width: 0; }
.oc-stn { flex: 0 0 1.4rem; font-weight: 700; }

.oc-time {
  flex: 0 0 3.4rem;
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 0.75rem;
  color: var(--text-muted);

  // Distinguishes "12m and counting" from "ended at 18:38" beyond the tooltip
  // alone: live rows get the accent color the LIVE badge already uses.
  .oc-row--live & { color: var(--accent); font-weight: 700; }
}

.oc-players { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.oc-round { flex: 0 0 8rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); }

.oc-score {
  flex: 0 0 auto;
  display: flex;
  align-items: baseline;
  gap: 0.4em;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.oc-bestof {
  font-size: 0.62rem;
  font-weight: 400;
  color: var(--text-muted);
  white-space: nowrap;
}

.oc-status {
  flex: 0 0 6rem;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 0.35em;
  padding: 0.15em 0.5em;
  border: 1px solid var(--line-subtle);
  border-radius: 999px;
  background: var(--surface-inset);
  color: var(--text-muted);
  text-transform: uppercase;
  font-size: 0.62rem;
  font-weight: 700;
  letter-spacing: 0.05em;

  &--live {
    color: var(--accent);
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 20%, transparent);
  }

  &--matched {
    color: var(--text-warning);
    border-color: var(--text-warning);
    background: color-mix(in srgb, var(--text-warning) 16%, transparent);
  }

  &--reported {
    color: var(--text-success);
    border-color: var(--text-success);
    background: color-mix(in srgb, var(--text-success) 14%, transparent);
  }
}

.oc-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  animation: oc-status-pulse 2s ease-out infinite;
}

@keyframes oc-status-pulse {
  0% { box-shadow: 0 0 0 0 color-mix(in srgb, currentColor 60%, transparent); }
  70% { box-shadow: 0 0 0 5px transparent; }
  100% { box-shadow: 0 0 0 0 transparent; }
}

.oc-row-detail {
  display: flex;
  align-items: flex-start;
  gap: 0.75rem;
  padding-left: 2rem;
}

// The per-game strip: this is the point of the rework, so each game is its
// own small chip (both characters, winner ring, loser dimmed) rather than a
// dense blob of icons.
.oc-strip {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 0.3rem;
  flex-wrap: wrap;
}

.oc-strip--empty {
  color: var(--text-muted);
  font-size: 0.72rem;
  font-style: italic;
}

.oc-game {
  display: inline-flex;
  align-items: center;
  gap: 0.2rem;
  padding: 0.15rem 0.35rem;
  border-radius: 0.5em;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
}

.oc-game-side {
  display: inline-flex;
  border-radius: 50%;
}

.oc-game-side--won {
  box-shadow: 0 0 0 2px var(--text-success);
}

.oc-game-side--lost {
  opacity: 0.4;
}

// The persistent start.gg-side mapping (station tag -> bracket entrant), one
// row per player -- not winner-only. Color + italics carry the confidence so
// a guess never reads as a confirmed fact at a glance.
.oc-match {
  flex: 0 0 auto;
  min-width: 9rem;
  max-width: 13rem;
  display: flex;
  flex-direction: column;
  gap: 0.1rem;
  font-size: 0.74rem;
  text-align: right;

  &--high { color: var(--text-success); }
  &--low { color: var(--text-warning); font-style: italic; }
  &--none, &--absent { color: var(--text-muted); font-style: italic; }
}

.oc-match-row {
  display: flex;
  justify-content: flex-end;
  align-items: baseline;
  gap: 0.3em;
  overflow: hidden;
  white-space: nowrap;
}

.oc-match-tag { font-weight: 600; overflow: hidden; text-overflow: ellipsis; }
.oc-match-arrow { color: var(--text-muted); font-style: normal; }
.oc-match-entrant { overflow: hidden; text-overflow: ellipsis; }

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

  /* The name-matched candidate leads, but it's still a guess -- emphasized,
     not pre-selected. */
  &--suggested { border-color: var(--accent); }
}

.oc-pick-label { font-size: 0.75rem; color: var(--text-muted); }
</style>
