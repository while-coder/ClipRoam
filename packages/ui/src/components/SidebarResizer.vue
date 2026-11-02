<script setup lang="ts">
import { computed, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from "vue";

const props = defineProps<{
  storageKey: string;
  label: string;
  defaultWidth: number;
  minWidth: number;
  maxWidth: number;
  contentMinWidth: number;
}>();
const emit = defineEmits<{ resize: [width: number] }>();
const handle = ref<HTMLElement>();
const preferredWidth = ref(props.defaultWidth);
const containerWidth = ref(Infinity);
const maximum = computed(() => Math.max(0, Math.min(props.maxWidth, containerWidth.value - props.contentMinWidth)));
const minimum = computed(() => Math.min(props.minWidth, maximum.value));
const width = computed(() => Math.round(Math.max(minimum.value, Math.min(maximum.value, preferredWidth.value))));
watch(width, (value) => emit("resize", value), { immediate: true });
let observer: ResizeObserver | undefined;
let drag: { pointerId: number; x: number; width: number } | undefined;
const dragging = ref(false);

function save(value: number): void {
  preferredWidth.value = Math.max(minimum.value, Math.min(maximum.value, value));
  try { localStorage.setItem(props.storageKey, String(preferredWidth.value)); } catch { /* Storage can be disabled. */ }
}
function start(event: PointerEvent): void {
  if (event.button !== 0 || !event.isPrimary) return;
  event.preventDefault();
  handle.value?.focus();
  handle.value?.setPointerCapture(event.pointerId);
  drag = { pointerId: event.pointerId, x: event.clientX, width: width.value };
  dragging.value = true;
}
function move(event: PointerEvent): void {
  if (drag?.pointerId !== event.pointerId) return;
  preferredWidth.value = Math.max(minimum.value, Math.min(maximum.value, drag.width + event.clientX - drag.x));
}
function stop(): void {
  if (!drag) return;
  const pointerId = drag.pointerId;
  drag = undefined;
  dragging.value = false;
  if (handle.value?.hasPointerCapture(pointerId)) handle.value.releasePointerCapture(pointerId);
  save(width.value);
}
function keydown(event: KeyboardEvent): void {
  const step = event.shiftKey ? 32 : 8;
  const value = { ArrowLeft: width.value - step, ArrowRight: width.value + step, Home: minimum.value, End: maximum.value }[event.key];
  if (value === undefined) return;
  event.preventDefault();
  event.stopPropagation();
  save(value);
}
onMounted(() => {
  try {
    const stored = Number(localStorage.getItem(props.storageKey));
    if (Number.isFinite(stored) && stored > 0) preferredWidth.value = Math.max(props.minWidth, Math.min(props.maxWidth, stored));
  } catch { /* Use the default when storage is unavailable. */ }
  const container = handle.value?.parentElement;
  if (!container) return;
  containerWidth.value = container.clientWidth;
  observer = new ResizeObserver(() => { containerWidth.value = container.clientWidth; });
  observer.observe(container);
});
onDeactivated(stop);
onBeforeUnmount(() => { stop(); observer?.disconnect(); });
</script>

<template>
  <div ref="handle" class="sidebar-resizer" :class="{ dragging }" :style="{ left: `${width}px` }"
    role="separator" tabindex="0" aria-orientation="vertical" :aria-label="label"
    :aria-valuemin="minimum" :aria-valuemax="maximum" :aria-valuenow="width" :aria-valuetext="`${width} 像素`"
    title="拖动调整宽度，双击恢复默认；方向键微调"
    @pointerdown="start" @pointermove="move" @pointerup="stop" @pointercancel="stop" @lostpointercapture="stop"
    @keydown="keydown" @dblclick="save(defaultWidth)">
    <span />
  </div>
</template>

<style scoped>
.sidebar-resizer { position: absolute; top: 0; bottom: 0; width: 8px; margin-left: -4px; z-index: 5; cursor: col-resize; touch-action: none; user-select: none; outline: none; }
.sidebar-resizer span { position: absolute; inset: 0 3px; background: transparent; transition: background-color 150ms; }
.sidebar-resizer:hover span, .sidebar-resizer:focus-visible span, .sidebar-resizer.dragging span { background: var(--sui-primary); }
.sidebar-resizer.dragging::after { content: ""; position: fixed; inset: 0; cursor: col-resize; }
@media (prefers-reduced-motion: reduce) { .sidebar-resizer span { transition: none; } }
</style>
