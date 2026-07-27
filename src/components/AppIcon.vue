<script setup lang="ts">
// Inline SVG icons, not a webfont: a tournament PC often has no internet, and
// the tag tool's titlebar already draws its glyph this way. Paths are
// Material-Symbols shapes so they sit next to the rest of the family.

const PATHS: Record<string, string> = {
  close:
    'M13.46 12L19 17.54V19h-1.46L12 13.46L6.46 19H5v-1.46L10.54 12L5 6.46V5h1.46L12 10.54L17.54 5H19v1.46z',
  back: 'M20 11H7.83l5.59-5.59L12 4l-8 8l8 8l1.41-1.41L7.83 13H20v-2z',
  swap: 'M6.99 11L3 15l3.99 4v-3H14v-2H6.99v-3zM21 9l-3.99-4v3H10v2h7.01v3L21 9z',
  settings:
    'M3 17v2h6v-2H3zM3 5v2h10V5H3zm10 16v-2h8v-2h-8v-2h-2v6h2zM7 9v2H3v2h4v2h2V9H7zm14 4v-2H11v2h10zm-6-4h2V7h4V5h-4V3h-2v6z',
  log: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z',
  check: 'M9 16.17L4.83 12l-1.42 1.41L9 19L21 7l-1.41-1.41z',
  warning: 'M1 21h22L12 2L1 21zm12-3h-2v-2h2v2zm0-4h-2v-4h2v4z',
  star: 'M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2L9.19 8.63L2 9.24l5.46 4.73L5.82 21z',
  pending:
    'M12 4a8 8 0 1 0 0 16a8 8 0 0 0 0-16zm0 14a6 6 0 1 1 0-12a6 6 0 0 1 0 12z',
  copy: 'M16 1H4a2 2 0 0 0-2 2v14h2V3h12V1zm3 4H8a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2zm0 16H8V7h11v14z',
  // Mode-card glyphs: a controller for "plays games", stacked rows for
  // "reads the console". "Both" simply shows the two together.
  gamepad:
    'M21 6H3a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h18a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2zm-10 7H8v3H6v-3H3v-2h3V8h2v3h3v2zm4.5 2a1.5 1.5 0 1 1 0-3a1.5 1.5 0 0 1 0 3zm4-3a1.5 1.5 0 1 1 0-3a1.5 1.5 0 0 1 0 3z',
  rows: 'M3 5h18v2H3zm0 6h18v2H3zm0 6h18v2H3z',
};

const props = withDefaults(defineProps<{ name: keyof typeof PATHS | string; size?: number }>(), {
  size: 16,
});
</script>

<template>
  <svg
    class="icon"
    :width="props.size"
    :height="props.size"
    viewBox="0 0 24 24"
    aria-hidden="true"
    focusable="false"
  >
    <path fill="currentColor" :d="PATHS[props.name] ?? ''" />
  </svg>
</template>

<style scoped>
.icon {
  flex-shrink: 0;
  display: block;
}
</style>
