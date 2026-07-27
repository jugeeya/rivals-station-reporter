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
</script>

<template>
  <div class="ls" :class="{ 'ls--idle': !live }">
    <template v-if="live">
      <div class="ls-head">
        <span class="ls-pulse" aria-hidden="true"></span>
        <span class="ls-label">Now playing</span>
        <span v-if="modeLabel" class="ls-mode" :title="'A ' + modeLabel + ' ladder game — never reported to the bracket'">
          {{ modeLabel }} — not a bracket set
        </span>
        <span class="ls-games">{{ live.games }} game{{ live.games === 1 ? '' : 's' }}</span>
      </div>
      <div class="ls-players">
        <template v-for="(p, i) in live.players" :key="p.tag + i">
          <div class="ls-player" :class="{ 'ls-player--lead': p.won }">
            <span class="ls-tag">{{ p.tag }}</span>
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

  &:last-child { text-align: right; }
  &--lead .ls-tag { color: var(--text-success); }
}

.ls-tag {
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 1.35rem;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
