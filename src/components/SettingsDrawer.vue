<script setup lang="ts">
// Everything editable after setup — same fields as onboarding plus the paths
// and tuning knobs. Saving hot-rebuilds the engine; nothing needs a restart.

import { onMounted, reactive, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import { invoke } from '@tauri-apps/api/core';
import { state, saveConfig } from '../lib/engine';
import { open } from '@tauri-apps/plugin-dialog';

const emit = defineEmits<{ close: [] }>();

const draft = reactive({ ...state.s.config });
const saving = ref(false);
const err = ref('');
const autostart = ref(false);

onMounted(async () => {
  try {
    autostart.value = await invoke<boolean>('get_autostart');
  } catch {
    /* browser mock */
  }
});

async function toggleAutostart() {
  autostart.value = !autostart.value;
  try {
    await invoke('set_autostart', { enabled: autostart.value });
  } catch (e) {
    err.value = String(e);
  }
}

async function pickDir(field: 'replays' | 'dir') {
  const chosen = await open({ directory: true, title: 'Choose folder' });
  if (typeof chosen === 'string') draft[field] = chosen;
}

async function pickSave() {
  const chosen = await open({
    title: 'Choose Rivals2_StatsSaveSlot.sav',
    filters: [{ name: '.sav file', extensions: ['sav'] }],
  });
  if (typeof chosen === 'string') draft.save = chosen;
}

async function save() {
  saving.value = true;
  err.value = '';
  try {
    await saveConfig({ ...draft, station: Number(draft.station) || 1 });
    emit('close');
  } catch (e) {
    err.value = String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="sd-backdrop" @click.self="emit('close')">
    <div class="sd" role="dialog" aria-modal="true" aria-label="Settings">
      <div class="sd-head">
        <span class="sd-title">Settings</span>
        <button class="icon-btn" title="Close" @click="emit('close')">
          <AppIcon name="close" :size="14" />
        </button>
      </div>

      <div class="sd-body">
        <label class="sd-field">
          <span>Mode</span>
          <select v-model="draft.mode" class="sd-input">
            <option value="station">station — report this PC's games</option>
            <option value="operator">operator — run the hub + console</option>
            <option value="both">both</option>
          </select>
        </label>

        <label v-if="draft.mode !== 'operator'" class="sd-field">
          <span>Station number</span>
          <input v-model.number="draft.station" type="number" min="0" max="99" class="sd-input sd-input--num" />
        </label>

        <label class="sd-field">
          <span>start.gg event (URL or slug; empty = local scoreboard)</span>
          <input v-model="draft.slug" type="text" class="sd-input" />
        </label>

        <label v-if="draft.mode === 'station'" class="sd-field">
          <span>Hub / broker URL</span>
          <input v-model="draft.broker" type="text" class="sd-input" />
        </label>

        <label class="sd-field">
          <span>Shared key (must match the operator's — required to send)</span>
          <input v-model="draft.key" type="password" class="sd-input" autocomplete="off" />
        </label>

        <label v-if="draft.mode !== 'station'" class="sd-field">
          <span>start.gg API token (operator only)</span>
          <input v-model="draft.startgg_token" type="password" class="sd-input" autocomplete="off" />
        </label>

        <template v-if="draft.mode !== 'operator'">
          <div class="sd-field">
            <span>Stats save (empty = auto-detect)</span>
            <div class="sd-row">
              <input v-model="draft.save" type="text" class="sd-input" :placeholder="state.s.health.savePath" />
              <button class="btn sd-pick" @click="pickSave">…</button>
            </div>
          </div>
          <div class="sd-field">
            <span>Replays folder (empty = auto-detect)</span>
            <div class="sd-row">
              <input v-model="draft.replays" type="text" class="sd-input" :placeholder="state.s.health.replaysPath" />
              <button class="btn sd-pick" @click="pickDir('replays')">…</button>
            </div>
          </div>
        </template>

        <details class="sd-advanced">
          <summary>Advanced</summary>
          <div class="sd-field">
            <span>Output folder (current/live/set files)</span>
            <div class="sd-row">
              <input v-model="draft.dir" type="text" class="sd-input" :placeholder="state.s.health.outDir" />
              <button class="btn sd-pick" @click="pickDir('dir')">…</button>
            </div>
          </div>
          <label class="sd-field">
            <span>Idle seconds before an open set finalizes</span>
            <input v-model.number="draft.idle" type="number" min="30" class="sd-input sd-input--num" />
          </label>
          <label v-if="draft.mode !== 'station'" class="sd-field">
            <span>Hub port</span>
            <input v-model.number="draft.hub_port" type="number" min="1" max="65535" class="sd-input sd-input--num" />
          </label>
          <label class="sd-check">
            <input v-model="draft.dry_run" type="checkbox" />
            <span>Dry run — log what would be sent instead of sending it</span>
          </label>
        </details>

        <label class="sd-check">
          <input type="checkbox" :checked="autostart" @change="toggleAutostart" />
          <span>Start with Windows (applies immediately)</span>
        </label>

        <p v-if="err" class="sd-err">{{ err }}</p>
      </div>

      <div class="sd-foot">
        <button class="btn btn-primary" :disabled="saving" @click="save">
          {{ saving ? 'Saving…' : 'Save' }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.sd-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  justify-content: flex-end;
  z-index: 40;
}

.sd {
  width: min(26rem, 92vw);
  height: 100%;
  background: var(--surface);
  border-left: 1px solid var(--line);
  display: flex;
  flex-direction: column;
}

.sd-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 1rem 1.25rem 0.5rem;
}

.sd-title { font-weight: 700; font-size: 1.05rem; }

.sd-body {
  flex: 1;
  overflow-y: auto;
  padding: 0.5rem 1.25rem;
  display: flex;
  flex-direction: column;
  gap: 0.8rem;
}

.sd-field {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  font-size: 0.8rem;

  > span { color: var(--text-muted); }
}

.sd-row { display: flex; gap: 0.35rem; }
.sd-pick { flex: 0 0 auto; width: auto; padding: 0.3em 0.7em; }

.sd-input {
  width: 100%;
  font-family: inherit;
  font-size: 0.85rem;
  color: var(--text-primary);
  background: rgba(0, 0, 0, 0.25);
  border: 1px solid var(--line);
  border-radius: var(--radius-button);
  padding: 0.4em 0.55em;

  &::placeholder { color: rgba(255, 255, 255, 0.3); }
  &:focus-visible { outline: 2px solid rgba(99, 102, 241, 0.6); outline-offset: 1px; }
  &--num { width: 7rem; }
}

.sd-check {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.8rem;
  color: var(--text-muted);
}

.sd-advanced {
  summary { cursor: pointer; font-size: 0.8rem; color: var(--text-muted); }
  display: flex;
  flex-direction: column;
  gap: 0.8rem;
}

.sd-err { margin: 0; color: var(--text-failure); font-size: 0.8rem; }

.sd-foot {
  padding: 0.75rem 1.25rem 1.25rem;
  border-top: 1px solid var(--line-subtle);
}
</style>
