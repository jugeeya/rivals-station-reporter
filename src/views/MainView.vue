<script setup lang="ts">
// The one screen after setup: health strip up top, the live set front and
// center, recent sets below; operators get the all-stations console and the
// "stations point here" banner. Settings and the log are slide-over drawers
// so the main view never reflows.

import { computed, ref } from 'vue';
import { useNowSeconds } from '../lib/useNow';
import AnimatedCard from '../components/AnimatedCard.vue';
import AppIcon from '../components/AppIcon.vue';
import HealthStrip from '../components/HealthStrip.vue';
import LiveSetCard from '../components/LiveSetCard.vue';
import StationSets from '../components/StationSets.vue';
import OperatorConsole from '../components/OperatorConsole.vue';
import CurrentSets from '../components/CurrentSets.vue';
import SettingsDrawer from '../components/SettingsDrawer.vue';
import { state } from '../lib/engine';

const showSettings = ref(false);
const showLog = ref(false);

const cfg = computed(() => state.s.config);
const isStation = computed(() => cfg.value.mode !== 'operator');
const isOperator = computed(() => cfg.value.mode !== 'station');

// Ticks on its own (see useNow.ts): this label is the one place a stalled
// engine shows up, so it must keep aging even when no new state arrives.
const nowS = useNowSeconds(10_000);
const statusAgo = computed(() => {
  const d = Math.max(0, Math.floor(nowS.value - state.s.status.t));
  return d < 2 ? 'just now' : d < 60 ? `${d}s ago` : `${Math.floor(d / 60)}m ago`;
});

async function copyHubUrl() {
  if (state.s.hubUrl) await navigator.clipboard.writeText(state.s.hubUrl);
}
</script>

<template>
  <AnimatedCard wide static>
    <div class="mv">
      <header class="mv-head">
        <div class="mv-title-wrap">
          <span class="mv-title">Rivals Station Reporter</span>
          <span v-if="isStation" class="mv-station">Station {{ cfg.station }}</span>
          <span class="mv-mode">{{ cfg.mode }}</span>
          <span v-if="cfg.dry_run" class="mv-dry">DRY-RUN</span>
        </div>
        <div class="mv-actions">
          <button class="linkish" :class="{ 'linkish--on': showLog }" @click="showLog = !showLog">
            <AppIcon name="log" />Log
          </button>
          <button class="linkish" @click="showSettings = true">
            <AppIcon name="settings" />Settings
          </button>
        </div>
      </header>

      <HealthStrip />

      <div v-if="isOperator && state.s.hubUrl" class="mv-hub">
        <span class="mv-hub-label">Stations point here:</span>
        <code class="mv-hub-url">{{ state.s.hubUrl }}</code>
        <button class="btn mv-hub-copy" @click="copyHubUrl">
          <AppIcon name="copy" :size="14" />Copy
        </button>
      </div>

      <template v-if="isStation">
        <LiveSetCard :live="state.s.snapshot.live" />
        <StationSets :snapshot="state.s.snapshot" />
      </template>

      <OperatorConsole v-if="isOperator" />
      <CurrentSets v-if="isOperator" />

      <div v-if="showLog" class="mv-log">
        <pre class="mv-log-pre">{{ state.s.log.join('\n') || '(nothing logged yet)' }}</pre>
      </div>

      <footer class="mv-foot">
        <span class="mv-dot" :class="{ 'mv-dot--err': state.s.status.error }" aria-hidden="true"></span>
        <span class="mv-status" :class="{ 'mv-status--err': state.s.status.error }">
          {{ state.s.status.msg }}
        </span>
        <span class="mv-ago">· {{ statusAgo }}</span>
        <span class="mv-event">{{ cfg.slug || 'no event, local scoreboard' }}</span>
      </footer>
    </div>

    <SettingsDrawer v-if="showSettings" @close="showSettings = false" />
  </AnimatedCard>
</template>

<style scoped lang="scss">
.mv {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

.mv-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.mv-title-wrap {
  display: flex;
  align-items: center;
  gap: 0.6rem;
}

.mv-title {
  font-size: 1.15rem;
  font-weight: 700;
  letter-spacing: 0.02em;
}

.mv-station {
  padding: 0.15em 0.6em;
  background: var(--accent);
  color: var(--on-accent, #fff);
  border-radius: var(--radius-button);
  font-size: 0.8rem;
  font-weight: 700;
}

.mv-mode {
  color: var(--text-muted);
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.08em;
}

.mv-dry {
  color: var(--text-warning);
  font-size: 0.75rem;
  font-weight: 700;
}

.mv-actions {
  display: flex;
  gap: 0.35rem;
}

/* The Log toggle stays lit while its panel is open. */
.linkish--on {
  background: var(--surface-hover);
  color: var(--text-primary);
}

.mv-hub {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding: 0.55rem 0.75rem;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-panel);
}

.mv-hub-label { font-size: 0.8rem; color: var(--text-muted); }

.mv-hub-url {
  flex: 1;
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 0.95rem;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mv-hub-copy {
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
  width: auto;
  padding: 0.35em 0.9em;
  font-size: 0.8rem;
}

.mv-log {
  background: rgba(0, 0, 0, 0.3);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-panel);
  max-height: 10rem;
  overflow-y: auto;
}

.mv-log-pre {
  margin: 0;
  padding: 0.5rem 0.7rem;
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 0.72rem;
  color: var(--text-muted);
  white-space: pre-wrap;
}

.mv-foot {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  font-size: 0.75rem;
  color: var(--text-muted);
}

.mv-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-success);

  &--err { background: var(--text-failure); }
}

.mv-status { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mv-status--err { color: var(--text-failure); }
.mv-ago { flex: 0 0 auto; }
.mv-event { margin-left: auto; flex: 0 0 auto; }
</style>
