<script lang="ts">
// A plain <script> block, deliberately: everything in <script setup> is
// compiled into setup() and re-runs on every mount, and this drawer is v-if'd
// so it remounts each time it opens. Only a real module-scope binding can
// remember that the update check already ran, which is what keeps opening
// Settings repeatedly from re-hitting the update endpoint each time.
let autoCheckedThisRun = false;
</script>

<script setup lang="ts">
// Everything editable after setup: same fields as onboarding plus the paths
// and tuning knobs. Saving hot-rebuilds the engine; nothing needs a restart.

import { computed, onMounted, reactive, ref } from 'vue';
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
  try {
    const { getVersion } = await import('@tauri-apps/api/app');
    currentVersion.value = await getVersion();
  } catch {
    /* browser mock */
  }
  // Check once per app run, so opening Settings surfaces a new release
  // without anyone having to think to press the button.
  if (!autoCheckedThisRun) {
    autoCheckedThisRun = true;
    void checkUpdate();
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

// ---- LAN hub discovery ------------------------------------------------------
// Sweeps the local /24 for a hub so the station doesn't need to be told the
// operator's IP. One result auto-connects (there is only ever one operator);
// several is unexpected — usually a stale hub still running on someone's
// laptop — so that case asks rather than guessing.

type FoundHub = { url: string; slug: string | null; startgg: boolean };

const scanning = ref(false);
const scanned = ref(false);
const hubs = ref<FoundHub[]>([]);

function hubLabel(h: FoundHub) {
  const event = h.slug || 'no event configured';
  return `${h.url} · ${event}${h.startgg ? '' : ', no start.gg token'}`;
}

function useHub(h: FoundHub) {
  draft.broker = h.url;
  hubs.value = [h];
}

async function findHubs() {
  scanning.value = true;
  scanned.value = false;
  err.value = '';
  hubs.value = [];
  try {
    const res = await invoke<{ hubs: FoundHub[] }>('find_hubs');
    hubs.value = res.hubs ?? [];
    // Exactly one: connect to it. The label below always names what was
    // picked, so a wrong auto-connect is visible and correctable rather than
    // a silent URL swap.
    if (hubs.value.length === 1) draft.broker = hubs.value[0].url;
    scanned.value = true;
  } catch (e) {
    err.value = String(e);
  } finally {
    scanning.value = false;
  }
}

// ---- updates -----------------------------------------------------------------
// Installing relaunches the app, which was previously refused while a set was
// open because the in-progress set only existed in memory and would have been
// lost. The open set is journalled to disk now and resumes on startup, well
// inside the staleness bound, so a mid-set update is safe and the block is
// gone. Installing is still a deliberate second click rather than automatic:
// dropping the window unannounced during a bracket is its own problem.

const updateState = ref<'idle' | 'checking' | 'none' | 'found' | 'installing'>('idle');
const updateVersion = ref('');
const currentVersion = ref('');
let pendingUpdate: { downloadAndInstall: () => Promise<void> } | null = null;

const setInProgress = computed(() => state.s.snapshot.live != null);

async function checkUpdate() {
  updateState.value = 'checking';
  err.value = '';
  try {
    const { check } = await import('@tauri-apps/plugin-updater');
    const up = await check();
    if (up) {
      pendingUpdate = up;
      updateVersion.value = up.version;
      updateState.value = 'found';
    } else {
      updateState.value = 'none';
    }
  } catch (e) {
    // A GitHub release is assembled by several jobs finishing independently;
    // if one platform hasn't uploaded yet the manifest briefly lacks it and
    // the updater plugin throws this exact message. Not something a click
    // can fix, so show it as "nothing to report yet" rather than an alarming
    // raw error. release.yml keeps the release itself a draft until every
    // platform has uploaded specifically to make this rare; this is a
    // fallback for whatever narrow window is left (e.g. GitHub's own
    // propagation), not the primary defense.
    const msg = String(e);
    if (/fallback platform/i.test(msg)) {
      updateState.value = 'none';
    } else {
      err.value = msg;
      updateState.value = 'idle';
    }
  }
}

async function installUpdate() {
  if (!pendingUpdate) return;
  updateState.value = 'installing';
  err.value = '';
  try {
    await pendingUpdate.downloadAndInstall();
    const { relaunch } = await import('@tauri-apps/plugin-process');
    await relaunch();
  } catch (e) {
    err.value = String(e);
    updateState.value = 'found';
  }
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
            <option value="station">station: report this PC's games</option>
            <option value="operator">operator: run the hub + console</option>
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

        <div v-if="draft.mode === 'station'" class="sd-field">
          <span>Hub / broker URL</span>
          <div class="sd-row">
            <input v-model="draft.broker" type="text" class="sd-input" />
            <button class="btn sd-find" :disabled="scanning" @click="findHubs">
              {{ scanning ? 'Scanning…' : 'Find hub' }}
            </button>
          </div>
          <p v-if="scanning" class="sd-hint">Checking every address on this network…</p>
          <p v-else-if="scanned && hubs.length === 1" class="sd-hint sd-hint--ok">
            Connected to {{ hubLabel(hubs[0]) }}
          </p>
          <p v-else-if="scanned && hubs.length === 0" class="sd-hint sd-hint--warn">
            No hub found on this network. Check the operator is running, or type the address above.
          </p>
          <template v-else-if="scanned && hubs.length > 1">
            <p class="sd-hint sd-hint--warn">
              Found {{ hubs.length }} hubs. Pick the right one:
            </p>
            <button
              v-for="h in hubs"
              :key="h.url"
              class="btn sd-hub-pick"
              :class="{ 'sd-hub-pick--on': draft.broker === h.url }"
              @click="useHub(h)"
            >
              {{ hubLabel(h) }}
            </button>
          </template>
        </div>

        <label class="sd-field">
          <span>Shared key (must match the operator's; required to send)</span>
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
            <input v-model="draft.dry_run" type="checkbox" class="sd-checkbox" />
            <span>Dry run: log what would be sent instead of sending it</span>
          </label>
        </details>

        <label class="sd-check">
          <input
            type="checkbox"
            class="sd-checkbox"
            :checked="autostart"
            @change="toggleAutostart"
          />
          <span>Start with Windows (applies immediately)</span>
        </label>

        <div class="sd-field">
          <span>Updates<template v-if="currentVersion"> (currently on {{ currentVersion }})</template></span>
          <div class="sd-row">
            <button
              class="btn sd-find"
              :disabled="updateState === 'checking' || updateState === 'installing'"
              @click="checkUpdate"
            >
              {{ updateState === 'checking' ? 'Checking…' : 'Check for updates' }}
            </button>
            <button
              v-if="updateState === 'found' || updateState === 'installing'"
              class="btn btn-primary sd-find"
              :disabled="updateState === 'installing'"
              @click="installUpdate"
            >
              {{ updateState === 'installing' ? 'Installing…' : `Install ${updateVersion}` }}
            </button>
          </div>
          <p v-if="updateState === 'checking'" class="sd-hint">Checking for a newer version…</p>
          <p v-else-if="updateState === 'none'" class="sd-hint sd-hint--ok">
            {{ currentVersion ? `${currentVersion} is the latest version.` : 'You are on the latest version.' }}
          </p>
          <p v-else-if="updateState === 'found' && setInProgress" class="sd-hint sd-hint--ok">
            Version {{ updateVersion }} is available. A set is in progress; installing restarts
            the app but the set is journalled and resumes.
          </p>
          <p v-else-if="updateState === 'found'" class="sd-hint sd-hint--ok">
            Version {{ updateVersion }} is available. Installing restarts the app.
          </p>
          <p v-else-if="updateState === 'installing'" class="sd-hint">
            Downloading and installing, the app will restart itself.
          </p>
        </div>

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
  // Opaque: this sits over live app content, and the translucent --surface
  // let the console's set rows and buttons show through the fields.
  background: var(--surface-solid);
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
  padding: 0.75rem 1.25rem 1rem;
  display: flex;
  flex-direction: column;
  // Has to clearly beat .sd-field's own 0.4rem label-to-input gap, or each
  // label reads as belonging to the field above it instead of below.
  gap: 1.3rem;
}

.sd-field {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  font-size: 0.8rem;

  > span {
    color: var(--text-muted);
    line-height: 1.35;
  }
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
  // Not `center`: these labels wrap to two lines at this drawer width, and
  // centring then floats the box against the middle of the text block.
  align-items: flex-start;
  gap: 0.55rem;
  font-size: 0.8rem;
  line-height: 1.35;
  color: var(--text-muted);
  cursor: pointer;

  > span { padding-top: 0.05rem; }
}

// The native control ignores the dark theme and renders as a stock white
// Windows checkbox, which is the one unstyled thing left in the drawer.
.sd-checkbox {
  appearance: none;
  flex: 0 0 auto;
  width: 1rem;
  height: 1rem;
  margin: 0;
  border: 1px solid var(--line);
  border-radius: 0.25em;
  background: rgba(0, 0, 0, 0.25);
  cursor: pointer;
  display: grid;
  place-content: center;
  transition: background 0.12s ease, border-color 0.12s ease;

  // Drawn rather than a glyph so it scales with the box and needs no font.
  &::before {
    content: '';
    width: 0.55rem;
    height: 0.3rem;
    border-left: 2px solid var(--color-bg);
    border-bottom: 2px solid var(--color-bg);
    transform: rotate(-45deg) translate(0.03rem, -0.04rem);
    opacity: 0;
  }

  &:hover { border-color: rgba(255, 255, 255, 0.25); }

  // Longhand, not the `background` shorthand: only the colour should change
  // here, and the shorthand would also reset background-image/position.
  &:checked {
    background-color: var(--text-success);
    border-color: var(--text-success);

    &::before { opacity: 1; }
  }

  &:focus-visible {
    outline: 2px solid rgba(99, 102, 241, 0.6);
    outline-offset: 1px;
  }
}

.sd-advanced {
  display: flex;
  flex-direction: column;
  gap: 1.3rem;

  summary {
    cursor: pointer;
    font-size: 0.8rem;
    color: var(--text-muted);
    // `gap` alone leaves the disclosure arrow sitting tight on the first
    // field once the section is open.
    margin-bottom: 0.15rem;
  }
}

.sd-err { margin: 0; color: var(--text-failure); font-size: 0.8rem; }

.sd-find { flex: 0 0 auto; width: auto; padding: 0.3em 0.7em; white-space: nowrap; }
.sd-hint { margin: 0.15rem 0 0; font-size: 0.75rem; color: var(--text-muted); }
.sd-hint--ok { color: var(--text-success); }
.sd-hint--warn { color: var(--text-warning); }
/* Only rendered in the unexpected multi-hub case, so it can afford to be a
   full-width stack rather than competing for room with the URL field. */
.sd-hub-pick {
  width: 100%;
  margin-top: 0.25rem;
  font-size: 0.75rem;
  text-align: left;
}
.sd-hub-pick--on { border-color: var(--accent); color: var(--text-success); }

.sd-foot {
  padding: 0.75rem 1.25rem 1.25rem;
  border-top: 1px solid var(--line-subtle);
}
</style>
