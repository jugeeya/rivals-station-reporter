<script setup lang="ts">
// The set being played right now, front and center: both tags, characters,
// score, and one dot per game. Online/ranked sets are visibly badged with why
// they'll never touch the bracket.

import { computed } from 'vue';
import type { SnapshotSet } from '../types';

const props = defineProps<{ live: SnapshotSet | null }>();

const reportable = computed(
  () => !props.live?.mode || props.live.mode.toUpperCase() === 'LOCAL',
);
const modeLabel = computed(() =>
  props.live?.mode && props.live.mode.toUpperCase() !== 'LOCAL'
    ? props.live.mode.toLowerCase()
    : '',
);

// The public tag database's guess at whose start.gg account typed this tag in
// game — nothing to do with the bracket. Weaker than the operator console's
// match cell (which means "found in this bracket"), so it must never borrow
// that cell's success/warning colors; see .ls-sgg below.
function sggTitle(tag: string, sgg: string): string {
  return `Public tag database: "${tag}" is registered to start.gg @${sgg}. Not a bracket match, just what was submitted.`;
}
</script>

<template>
  <div class="ls" :class="{ 'ls--idle': !live }">
    <template v-if="live">
      <div class="ls-head">
        <span class="ls-pulse" aria-hidden="true"></span>
        <span class="ls-label">Now playing</span>
        <span v-if="modeLabel" class="ls-mode" :title="'A ' + modeLabel + ' ladder game, never reported to the bracket'">
          {{ modeLabel }}: not a bracket set
        </span>
        <span class="ls-games">{{ live.games }} game{{ live.games === 1 ? '' : 's' }}</span>
      </div>
      <div class="ls-players">
        <template v-for="(p, i) in live.players" :key="p.tag + i">
          <div class="ls-player" :class="{ 'ls-player--lead': p.won }">
            <div class="ls-tag-row">
              <span class="ls-tag">{{ p.tag }}</span>
              <span v-if="p.sgg" class="ls-sgg" :title="sggTitle(p.tag, p.sgg)">@{{ p.sgg }}</span>
            </div>
            <span class="ls-char">{{ p.char }}</span>
          </div>
          <div v-if="i === 0" class="ls-score">
            <span>{{ live.players[0]?.wins ?? 0 }}</span>
            <span class="ls-score-sep">–</span>
            <span>{{ live.players[1]?.wins ?? 0 }}</span>
          </div>
        </template>
      </div>
      <div v-if="!reportable" class="ls-note">Shown here, kept out of start.gg.</div>
    </template>
    <template v-else>
      <span class="ls-idle-text">Waiting for a game…</span>
    </template>
  </div>
</template>

<style scoped lang="scss">
.ls {
  padding: 0.9rem 1rem;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-panel);
  display: flex;
  flex-direction: column;
  gap: 0.7rem;

  &--idle {
    align-items: center;
    padding: 1.4rem;
  }
}

.ls-idle-text { color: var(--text-muted); font-size: 0.9rem; }

.ls-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.75rem;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.08em;
}

.ls-pulse {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--text-success);
  animation: ls-pulse 2s ease-out infinite;
}

@keyframes ls-pulse {
  0% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--text-success) 60%, transparent); }
  70% { box-shadow: 0 0 0 7px transparent; }
  100% { box-shadow: 0 0 0 0 transparent; }
}

.ls-mode {
  color: var(--text-warning);
  text-transform: none;
  letter-spacing: normal;
}

.ls-games { margin-left: auto; text-transform: none; letter-spacing: normal; }

.ls-players {
  display: grid;
  grid-template-columns: 1fr auto 1fr;
  align-items: center;
  gap: 1rem;
}

.ls-player {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  min-width: 0;

  &:last-child {
    text-align: right;
    .ls-tag-row { justify-content: flex-end; }
  }
  &--lead .ls-tag { color: var(--text-success); }
}

.ls-tag-row {
  display: flex;
  align-items: baseline;
  gap: 0.4em;
  min-width: 0;
}

.ls-tag {
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 1.35rem;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 0 1 auto;
}

// The public tag database's handle guess for this tag — a much weaker claim
// than the operator console's bracket match cell (.oc-match), so it must
// never borrow that cell's success/warning colors. Muted and small; a hint,
// not a fact.
.ls-sgg {
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 0.7rem;
  font-weight: 400;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 0 1 auto;
}

.ls-char { font-size: 0.8rem; color: var(--text-muted); }

.ls-score {
  display: flex;
  align-items: baseline;
  gap: 0.35rem;
  font-size: 2rem;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
}

.ls-score-sep { color: var(--text-muted); font-size: 1.2rem; }

.ls-note { font-size: 0.75rem; color: var(--text-warning); }
</style>
