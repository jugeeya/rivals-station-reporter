<script setup lang="ts">
// A character's 50px stock icon (public/characters/<slug>.png, see
// public/characters/ATTRIBUTION.md for provenance). Falls back to the
// character's name as text when the file is missing -- a future character
// will ship in station data before its icon lands in this app, and a broken
// image would be a worse failure mode than plain text.

import { computed, ref, watch } from 'vue';

const props = withDefaults(defineProps<{ character: string | null | undefined; size?: number }>(), {
  size: 20,
});

function slugify(name: string): string {
  return name.trim().toLowerCase().replace(/\s+/g, '-');
}

const label = computed(() => props.character?.trim() || '?');
const src = computed(() => (props.character?.trim() ? `/characters/${slugify(props.character)}.png` : ''));

// Flips once if the image 404s; reset whenever the character prop changes so
// a recycled DOM node (v-for over games) doesn't stay stuck showing the
// previous character's fallback text.
const broken = ref(false);
watch(
  () => props.character,
  () => {
    broken.value = false;
  },
);
</script>

<template>
  <img
    v-if="src && !broken"
    class="char-icon"
    :src="src"
    :width="size"
    :height="size"
    :alt="label"
    :title="label"
    @error="broken = true"
  />
  <span
    v-else
    class="char-icon char-icon--text"
    :title="label"
    :style="{ width: size + 'px', height: size + 'px', fontSize: Math.max(8, size * 0.4) + 'px' }"
  >{{ label }}</span>
</template>

<style scoped>
.char-icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
}

.char-icon--text {
  background: var(--surface-inset);
  border: 1px solid var(--line-subtle);
  color: var(--text-muted);
  font-weight: 700;
  text-transform: uppercase;
  overflow: hidden;
  white-space: nowrap;
  line-height: 1;
}
</style>
