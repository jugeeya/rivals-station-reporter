<script setup lang="ts">
// A themed replacement for the native <select>: the closed state can be
// restyled with `appearance: none`, but the OPEN option list is drawn by the
// OS/browser and cannot be reached with CSS in any broadly-supported way.
// This renders its own list instead, so the whole control matches the rest
// of the app's dark theme.
//
// Teleported to <body> and positioned with `position: fixed` from the
// trigger's own bounding rect, rather than a plain `position: absolute`
// child: every caller so far sits inside a scrolling list
// (CurrentSets.vue's `.as-list`, `max-height` + `overflow-y: auto`), and an
// absolutely-positioned popup would be clipped by that ancestor's overflow
// the moment the row scrolls near the bottom. Teleporting escapes that
// clipping and any stacking-context surprises (e.g. sitting under a drawer's
// backdrop) the same way a native select's OS-drawn popup does.

import { computed, nextTick, onBeforeUnmount, ref } from 'vue';

export interface DropdownOption {
  value: string | null;
  label: string;
}

const props = withDefaults(
  defineProps<{
    modelValue: string | null;
    options: DropdownOption[];
    placeholder?: string;
    disabled?: boolean;
  }>(),
  { placeholder: 'Select…', disabled: false },
);

const emit = defineEmits<{ 'update:modelValue': [string | null] }>();

const open = ref(false);
const highlighted = ref(0);
const rootEl = ref<HTMLElement | null>(null);
const triggerEl = ref<HTMLButtonElement | null>(null);
const listEl = ref<HTMLUListElement | null>(null);
const listStyle = ref<Record<string, string>>({});

const selectedLabel = computed(() => {
  const opt = props.options.find((o) => o.value === props.modelValue);
  return opt ? opt.label : props.placeholder;
});

function computePosition() {
  const r = triggerEl.value?.getBoundingClientRect();
  if (!r) return;
  // Flips above the trigger when there isn't room below -- the same call a
  // native popup makes near the bottom of the window.
  const spaceBelow = window.innerHeight - r.bottom;
  const openUp = spaceBelow < 200 && r.top > spaceBelow;
  listStyle.value = {
    position: 'fixed',
    left: `${r.left}px`,
    minWidth: `${r.width}px`,
    ...(openUp ? { bottom: `${window.innerHeight - r.top + 4}px` } : { top: `${r.bottom + 4}px` }),
  };
}

function onReposition() {
  // A scroll or resize can move the trigger anywhere relative to a fixed
  // popup; closing is simpler and just as unsurprising as a native select
  // doing the same when the page moves under it.
  close();
}

function openList() {
  if (props.disabled || !props.options.length) return;
  computePosition();
  open.value = true;
  const idx = props.options.findIndex((o) => o.value === props.modelValue);
  highlighted.value = idx >= 0 ? idx : 0;
  nextTick(scrollHighlightedIntoView);
  window.addEventListener('mousedown', onDocMouseDown, true);
  window.addEventListener('keydown', onKeydown, true);
  window.addEventListener('scroll', onReposition, true);
  window.addEventListener('resize', onReposition, true);
}

function close() {
  if (!open.value) return;
  open.value = false;
  window.removeEventListener('mousedown', onDocMouseDown, true);
  window.removeEventListener('keydown', onKeydown, true);
  window.removeEventListener('scroll', onReposition, true);
  window.removeEventListener('resize', onReposition, true);
}

function toggle() {
  if (open.value) close();
  else openList();
}

function onDocMouseDown(e: MouseEvent) {
  const target = e.target as Node;
  if (rootEl.value?.contains(target) || listEl.value?.contains(target)) return;
  close();
}

function select(opt: DropdownOption) {
  emit('update:modelValue', opt.value);
  close();
}

function scrollHighlightedIntoView() {
  const el = listEl.value?.children[highlighted.value] as HTMLElement | undefined;
  el?.scrollIntoView({ block: 'nearest' });
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    close();
    e.preventDefault();
  } else if (e.key === 'ArrowDown') {
    highlighted.value = Math.min(highlighted.value + 1, props.options.length - 1);
    scrollHighlightedIntoView();
    e.preventDefault();
  } else if (e.key === 'ArrowUp') {
    highlighted.value = Math.max(highlighted.value - 1, 0);
    scrollHighlightedIntoView();
    e.preventDefault();
  } else if (e.key === 'Enter' || e.key === ' ') {
    const opt = props.options[highlighted.value];
    if (opt) select(opt);
    e.preventDefault();
  }
}

onBeforeUnmount(close);
</script>

<template>
  <div class="dd" ref="rootEl">
    <button
      type="button"
      ref="triggerEl"
      class="dd-trigger"
      :class="{ 'dd-trigger--placeholder': modelValue == null }"
      :disabled="disabled"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="toggle"
    >
      <span class="dd-label">{{ selectedLabel }}</span>
      <svg class="dd-chevron" viewBox="0 0 24 24" aria-hidden="true">
        <path fill="currentColor" d="M7 10l5 5 5-5z" />
      </svg>
    </button>
    <Teleport to="body">
      <ul v-if="open" ref="listEl" class="dd-list" role="listbox" :style="listStyle">
        <li
          v-for="(opt, i) in options"
          :key="opt.value ?? '\0none'"
          role="option"
          :aria-selected="opt.value === modelValue"
          class="dd-option"
          :class="{ 'dd-option--active': i === highlighted, 'dd-option--selected': opt.value === modelValue }"
          @mouseenter="highlighted = i"
          @click="select(opt)"
        >
          {{ opt.label }}
        </li>
      </ul>
    </Teleport>
  </div>
</template>

<style scoped lang="scss">
.dd {
  display: inline-flex;
}

// Same look as the native picker it replaces (see CurrentSets.vue's old
// `.as-picker`), just with the arrow drawn by AppIcon-style inline SVG
// instead of a CSS background-image, since the whole option list now lives
// in this component rather than being left to the browser.
.dd-trigger {
  display: inline-flex;
  align-items: center;
  gap: 0.4em;
  background-color: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  border-radius: var(--radius-button);
  color: var(--text-primary);
  font: inherit;
  font-size: 0.75rem;
  padding: 0.25em 0.5em 0.25em 0.45em;
  cursor: pointer;

  &--placeholder {
    color: var(--text-muted);
  }
  &:focus-visible {
    outline: 2px solid rgba(99, 102, 241, 0.6);
    outline-offset: 1px;
  }
  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
}

.dd-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 9rem;
}

.dd-chevron {
  flex-shrink: 0;
  width: 0.85em;
  height: 0.85em;
  color: rgba(255, 255, 255, 0.6);
}

// Teleported to <body>, so this is the one place in the app that has to
// carry its own solid background/border/shadow rather than relying on an
// ancestor's -- there is no themed ancestor to inherit from out here.
.dd-list {
  z-index: 200;
  max-height: 16rem;
  overflow-y: auto;
  margin: 0;
  padding: 0.25rem;
  list-style: none;
  background: var(--surface-solid);
  border: 1px solid var(--line);
  border-radius: var(--radius-button);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  font-size: 0.75rem;
}

.dd-option {
  padding: 0.35em 0.6em;
  border-radius: calc(var(--radius-button) - 2px);
  color: var(--text-primary);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;

  &--active {
    background: rgba(99, 102, 241, 0.25);
  }
  &--selected {
    color: var(--accent);
    font-weight: 600;
  }
}
</style>
