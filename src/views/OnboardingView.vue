<script setup lang="ts">
// First-run setup: pick what this PC is, then fill in only what can't be
// auto-detected. Everything detected is shown with a check; everything
// entered is validated inline (the event URL echoes back what it points at),
// so a wrong paste is caught here instead of at the first set.

import { ref, computed, onMounted } from 'vue';
import AnimatedCard from '../components/AnimatedCard.vue';
import AppIcon from '../components/AppIcon.vue';
import { state, saveConfig, resolveEvent, defaultPaths } from '../lib/engine';
import type { EventSummary } from '../types';

const MODES = [
  {
    id: 'station' as const,
    title: 'Station',
    desc: 'This PC runs Rivals 2 at an event. Watches the game and reports each set to the operator.',
    icons: ['gamepad'],
  },
  {
    id: 'operator' as const,
    title: 'Operator',
    desc: 'The TO machine. Runs the hub every station reports to, and is the only PC that talks to start.gg.',
    icons: ['rows'],
  },
  {
    id: 'both' as const,
    title: 'Both',
    desc: 'One PC doing both jobs — plays games and runs the hub.',
    icons: ['gamepad', 'rows'],
  },
];

const step = ref<1 | 2>(1);
const mode = ref<'station' | 'operator' | 'both'>('station');

const station = ref(1);
const eventUrl = ref('');
const eventInfo = ref<EventSummary | null>(null);
const eventError = ref('');
const resolving = ref(false);
const key = ref('');
const brokerUrl = ref('');
const token = ref('');
const saving = ref(false);

const paths = ref({ save: '', saveExists: false, replays: '', replaysExists: false });
onMounted(async () => {
  paths.value = await defaultPaths();
  brokerUrl.value = state.s.config.broker;
});

const isStation = computed(() => mode.value !== 'operator');
const isOperator = computed(() => mode.value !== 'station');

async function checkEvent() {
  eventInfo.value = null;
  eventError.value = '';
  if (!eventUrl.value.trim()) return;
  resolving.value = true;
  try {
    eventInfo.value = await resolveEvent(eventUrl.value);
  } catch (e) {
    eventError.value = String(e);
  } finally {
    resolving.value = false;
  }
}

const canFinish = computed(() => {
  // An event is optional (local scoreboard works without one), but sending
  // needs the key; the operator additionally wants a token for start.gg.
  if (isStation.value && !station.value && station.value !== 0) return false;
  return true;
});

async function finish() {
  saving.value = true;
  try {
    await saveConfig({
      ...state.s.config,
      mode: mode.value,
      station: Number(station.value) || 1,
      slug: eventInfo.value?.slug ?? '',
      broker: brokerUrl.value.trim() || state.s.config.broker,
      key: key.value.trim(),
      startgg_token: token.value.trim(),
      configured: true,
    });
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <AnimatedCard static>
    <div class="ob">
      <template v-if="step === 1">
        <h1 class="ob-title">Rivals Station Reporter</h1>
        <p class="ob-sub">What is this PC at your event?</p>
        <div class="ob-modes">
          <button
            v-for="m in MODES"
            :key="m.id"
            class="ob-mode"
            :class="{ 'ob-mode--active': mode === m.id }"
            @click="mode = m.id"
          >
            <span class="ob-mode-icon">
              <AppIcon v-for="n in m.icons" :key="n" :name="n" :size="18" />
            </span>
            <span class="ob-mode-title">{{ m.title }}</span>
            <span class="ob-mode-desc">{{ m.desc }}</span>
          </button>
        </div>
        <button class="btn btn-primary" @click="step = 2">Continue</button>
      </template>

      <template v-else>
        <h1 class="ob-title">Set it up</h1>

        <div v-if="isStation" class="ob-field">
          <label>Station number</label>
          <input v-model.number="station" type="number" min="0" max="99" class="ob-input ob-input--num" />
          <p class="ob-help">The start.gg station this setup is assigned to.</p>
        </div>

        <div class="ob-field">
          <label>start.gg event <span class="ob-opt">(optional — without one it's a local scoreboard)</span></label>
          <div class="ob-row">
            <input
              v-model="eventUrl"
              type="text"
              class="ob-input"
              placeholder="Paste a start.gg link…"
              @keydown.enter="checkEvent"
            />
            <button class="btn" :disabled="resolving || !eventUrl.trim()" @click="checkEvent">
              {{ resolving ? '…' : 'Check' }}
            </button>
          </div>
          <p v-if="eventInfo" class="ob-help ob-help--ok">
            <AppIcon name="check" :size="13" />
            {{ eventInfo.tournament }} — {{ eventInfo.name }}
            <template v-if="eventInfo.entrants != null"> · {{ eventInfo.entrants }} entrants</template>
          </p>
          <p v-else-if="eventError" class="ob-help ob-help--err">{{ eventError }}</p>
        </div>

        <div v-if="mode === 'station'" class="ob-field">
          <label>Hub / broker URL</label>
          <input v-model="brokerUrl" type="text" class="ob-input" placeholder="http://192.168.…:8787 (from the operator's screen)" />
          <p class="ob-help">Shown big on the operator's screen — or leave the cloud broker default.</p>
        </div>

        <div class="ob-field">
          <label>Shared key <span class="ob-opt">(required to send — ask whoever runs the event)</span></label>
          <input v-model="key" type="password" class="ob-input" autocomplete="off" />
        </div>

        <div v-if="isOperator" class="ob-field">
          <label>start.gg API token <span class="ob-opt">(operator only — stays on this machine)</span></label>
          <input v-model="token" type="password" class="ob-input" autocomplete="off" />
        </div>

        <div v-if="isStation" class="ob-detect">
          <div class="ob-detect-row">
            <AppIcon :name="paths.saveExists ? 'check' : 'warning'" :size="13"
                  :class="paths.saveExists ? 'ok' : 'warn'" />
            <span class="ob-detect-label">Stats save</span>
            <span class="ob-detect-path" :title="paths.save">{{ paths.save }}</span>
          </div>
          <div class="ob-detect-row">
            <AppIcon :name="paths.replaysExists ? 'check' : 'warning'" :size="13"
                  :class="paths.replaysExists ? 'ok' : 'warn'" />
            <span class="ob-detect-label">Replays</span>
            <span class="ob-detect-path" :title="paths.replays">{{ paths.replays }}</span>
          </div>
          <p v-if="!paths.saveExists" class="ob-help ob-help--warn">
            Save not found — has Rivals 2 been run on this PC? (Paths can be changed later in Settings.)
          </p>
        </div>

        <div class="ob-actions">
          <button class="linkish" @click="step = 1">
            <AppIcon name="back" />Back
          </button>
          <button class="btn btn-primary ob-start" :disabled="!canFinish || saving" @click="finish">
            {{ saving ? 'Starting…' : 'Start' }}
          </button>
        </div>
      </template>
    </div>
  </AnimatedCard>
</template>

<style scoped lang="scss">
.ob {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 0.9rem;
}

.ob-title {
  margin: 0;
  font-size: 1.4rem;
  letter-spacing: 0.02em;
}

.ob-sub {
  margin: 0;
  color: var(--text-muted);
}

.ob-modes {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 0.6rem;
}

.ob-mode {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  padding: 0.85rem;
  text-align: left;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-panel);
  color: var(--text-primary);
  cursor: pointer;

  &:hover { border-color: var(--line); }
  &--active { border-color: var(--accent); outline: 1px solid var(--accent); }
}

.ob-mode-icon {
  display: flex;
  gap: 0.3rem;
  color: var(--text-muted);
}

.ob-mode--active .ob-mode-icon { color: var(--accent); }
.ob-mode-title { font-weight: 700; }
.ob-mode-desc { font-size: 0.75rem; color: var(--text-muted); }

.ob-field {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;

  label { font-size: 0.85rem; }
}

.ob-opt { color: var(--text-muted); font-size: 0.8em; }

.ob-row { display: flex; gap: 0.4rem; .btn { flex: 0 0 auto; width: auto; padding-inline: 1em; } }

.ob-input {
  width: 100%;
  font-family: inherit;
  font-size: 0.9rem;
  color: var(--text-primary);
  background: rgba(0, 0, 0, 0.25);
  border: 1px solid var(--line);
  border-radius: var(--radius-button);
  padding: 0.45em 0.6em;

  &::placeholder { color: rgba(255, 255, 255, 0.35); }
  &:focus-visible { outline: 2px solid rgba(99, 102, 241, 0.6); outline-offset: 1px; }
  &--num { width: 6rem; }
}

.ob-help {
  margin: 0;
  display: flex;
  align-items: center;
  gap: 0.3em;
  font-size: 0.75rem;
  color: var(--text-muted);
}
.ob-help--ok { color: var(--text-success); }
.ob-help--err { color: var(--text-failure); }
.ob-help--warn { color: var(--text-warning); }

.ob-detect {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  padding: 0.6rem 0.75rem;
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-panel);
}

.ob-detect-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.78rem;

  .ok { color: var(--text-success); }
  .warn { color: var(--text-warning); }
}

.ob-detect-label { flex: 0 0 auto; }
.ob-detect-path {
  color: var(--text-muted);
  font-family: 'Ubuntu Sans Mono Variable', monospace;
  font-size: 0.72rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ob-actions {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

/* Sits beside Back, so it sizes to its label instead of the full row. */
.ob-start {
  width: auto;
  padding-inline: 1.8em;
}
</style>
