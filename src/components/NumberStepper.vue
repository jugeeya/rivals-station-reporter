<script setup lang="ts">
// A themed replacement for a plain `<input type="number">`'s spin buttons:
// Chromium (WebView2, what this app actually runs on) draws them as a
// plain OS-grey up/down pair that ignores the dark theme entirely, the same
// problem DestinationDropdown.vue solved for the native <select> arrow. The
// native buttons are hidden via the `::-webkit-*-spin-button` pseudo-elements
// (the only thing Chromium lets CSS reach on them) and replaced with two
// small buttons drawn with the same chevron already used elsewhere in this
// app, so incrementing/decrementing still works but looks native to the rest
// of the UI instead of to Windows.

const props = withDefaults(
  defineProps<{ modelValue: number; min?: number; max?: number; step?: number }>(),
  { step: 1 },
);

const emit = defineEmits<{ 'update:modelValue': [number] }>();

function clamp(n: number): number {
  let v = n;
  if (props.min != null && v < props.min) v = props.min;
  if (props.max != null && v > props.max) v = props.max;
  return v;
}

function bump(delta: number) {
  const base = Number.isFinite(props.modelValue) ? props.modelValue : (props.min ?? 0);
  emit('update:modelValue', clamp(base + delta));
}

function onInput(e: Event) {
  const n = Number((e.target as HTMLInputElement).value);
  emit('update:modelValue', Number.isFinite(n) ? n : (props.min ?? 0));
}
</script>

<template>
  <div class="ns">
    <input
      class="ns-input"
      type="number"
      :value="modelValue"
      :min="min"
      :max="max"
      :step="step"
      @input="onInput"
    />
    <div class="ns-controls">
      <button type="button" class="ns-btn" tabindex="-1" title="Increase" @click="bump(step)">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M7 14l5-5 5 5z" /></svg>
      </button>
      <button type="button" class="ns-btn" tabindex="-1" title="Decrease" @click="bump(-step)">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M7 10l5 5 5-5z" /></svg>
      </button>
    </div>
  </div>
</template>

<style scoped lang="scss">
.ns {
  position: relative;
  display: inline-flex;
  width: 7rem;
}

.ns-input {
  width: 100%;
  font: inherit;
  font-size: 0.85rem;
  color: var(--text-primary);
  background: rgba(0, 0, 0, 0.25);
  border: 1px solid var(--line);
  border-radius: var(--radius-button);
  padding: 0.4em 1.6em 0.4em 0.55em;

  &::-webkit-inner-spin-button,
  &::-webkit-outer-spin-button {
    appearance: none;
    margin: 0;
  }
  &:focus-visible {
    outline: 2px solid rgba(99, 102, 241, 0.6);
    outline-offset: 1px;
  }
}

.ns-controls {
  position: absolute;
  right: 1px;
  top: 1px;
  bottom: 1px;
  width: 1.4rem;
  display: flex;
  flex-direction: column;
  border-left: 1px solid var(--line-subtle);
  border-radius: 0 calc(var(--radius-button) - 1px) calc(var(--radius-button) - 1px) 0;
  overflow: hidden;
}

.ns-btn {
  flex: 1 1 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: none;
  border-bottom: 1px solid var(--line-subtle);
  background: rgba(255, 255, 255, 0.03);
  color: rgba(255, 255, 255, 0.6);
  cursor: pointer;

  &:last-child {
    border-bottom: none;
  }
  &:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-primary);
  }
  &:active {
    background: rgba(255, 255, 255, 0.14);
  }

  svg {
    width: 0.7rem;
    height: 0.7rem;
    flex-shrink: 0;
  }
}
</style>
