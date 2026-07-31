// Wall-clock "now" that ticks while the component is mounted — one shared
// pattern for every elapsed/"ago" label. Reading Date.now() inside a computed
// is NOT reactive: the label would only update when some other dependency
// changed, which is exactly backwards for staleness indicators (the engine
// going quiet is the one case where nothing else changes — and the one case
// the label exists to reveal).

import { onMounted, onUnmounted, ref, type Ref } from 'vue';

export function useNowSeconds(intervalMs = 30_000): Ref<number> {
  const nowS = ref(Date.now() / 1000);
  let timer: ReturnType<typeof setInterval> | undefined;
  onMounted(() => {
    timer = setInterval(() => {
      nowS.value = Date.now() / 1000;
    }, intervalMs);
  });
  onUnmounted(() => {
    if (timer) clearInterval(timer);
  });
  return nowS;
}
