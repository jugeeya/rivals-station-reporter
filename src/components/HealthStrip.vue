<script setup lang="ts">
// The at-a-glance answer to "is everything working?" — each chip is one
// subsystem, with the failure text saying what to actually do about it.

import { computed } from 'vue';
import AppIcon from './AppIcon.vue';
import { state } from '../lib/engine';

interface Chip {
  label: string;
  ok: boolean;
  warn?: boolean;
  detail: string;
}

const chips = computed<Chip[]>(() => {
  const s = state.s;
  const cfg = s.config;
  const out: Chip[] = [];
  if (cfg.mode !== 'operator') {
    out.push({
      label: 'Save',
      ok: s.health.saveArmed,
      detail: s.health.saveArmed
        ? `Watching ${s.health.savePath}`
        : s.health.saveExists
          ? 'Save found but not read yet'
          : 'Save not found. Has Rivals 2 been run on this PC? Fix the path in Settings.',
    });
    out.push({
      label: 'Replays',
      ok: s.health.replaysExists,
      detail: s.health.replaysExists
        ? s.health.replaysPath
        : 'Replays folder not found. Timestamps and slots degrade without it.',
    });
    out.push({
      label: 'Sending',
      ok: !!(cfg.slug && (cfg.broker || s.hubUrl)),
      warn: !cfg.slug,
      detail: cfg.slug
        ? `Reporting to ${s.hubUrl ?? cfg.broker}`
        : 'No event configured. Local scoreboard only, nothing is sent.',
    });
  }
  if (cfg.mode !== 'station') {
    out.push({
      label: 'Hub',
      ok: !!s.hubUrl,
      detail: s.hubUrl ? `Serving ${s.hubUrl}` : 'Hub not running. Check the log.',
    });
    out.push({
      label: 'start.gg',
      ok: !!cfg.startgg_token,
      warn: !cfg.startgg_token,
      detail: cfg.startgg_token
        ? 'Token configured. Live scores and reports go to the bracket.'
        : 'No API token. Sets are tracked but nothing reaches start.gg.',
    });
  }
  return out;
});
</script>

<template>
  <div class="hs">
    <span
      v-for="c in chips"
      :key="c.label"
      class="hs-chip"
      :class="c.ok ? 'hs-chip--ok' : c.warn ? 'hs-chip--warn' : 'hs-chip--err'"
      :title="c.detail"
    >
      <AppIcon class="hs-icon" :name="c.ok ? 'check' : c.warn ? 'pending' : 'warning'" :size="13" />
      {{ c.label }}
    </span>
  </div>
</template>

<style scoped lang="scss">
.hs {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem;
}

.hs-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
  padding: 0.25em 0.7em;
  border-radius: var(--radius-button);
  border: 1px solid var(--line-subtle);
  background: var(--surface-inset);
  font-size: 0.78rem;
  cursor: default;

  &--ok .hs-icon { color: var(--text-success); }
  &--warn { color: var(--text-muted); .hs-icon { color: var(--text-warning); } }
  &--err { .hs-icon { color: var(--text-failure); } }
}
</style>
