<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { emit, emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow, LogicalSize, PhysicalPosition } from "@tauri-apps/api/window";
import { Copy, Download, Pin, X, Check, MousePointer2, SlidersHorizontal } from "lucide-vue-next";
import StickyFiles from "./StickyFiles.vue";
import { STICKY_ACTION, STICKY_CONTENT, STICKY_RESULT, STICKY_INPUT, STICKY_OPACITY, STICKY_OPACITY_PREVIEW, type StickyAction, type StickyPayload, type StickyInputMode } from "./messages";

document.documentElement.classList.add("sticky-window-root");

const current = getCurrentWindow();
const payload = ref<StickyPayload>();
const busy = ref(false);
const error = ref("");
const onTop = ref(false);
const OPACITY_STORAGE_KEY = "cliproam.sticky.opacity";
function readOpacity(): number {
  try {
    const saved = localStorage.getItem(OPACITY_STORAGE_KEY);
    const value = saved === null ? 78 : Number(saved);
    return Number.isFinite(value) && value >= 0 && value <= 100 ? value : 78;
  } catch { return 78; }
}
const opacity = ref(readOpacity());
const appearanceOpen = ref(false);
const inputMode = ref<StickyInputMode>({ interactive: false, altSupported: true, manualEdit: false });
const clickThrough = computed(() => !inputMode.value.interactive);
const previewOwner = ref<string>();
const displayOpacity = computed(() => inputMode.value.interactive && !previewOwner.value ? 1 : opacity.value / 100);
const appearanceStyle = computed(() => ({
  "--sticky-background-opacity": String(displayOpacity.value),
  "--sticky-content-opacity": String(0.35 + displayOpacity.value * 0.65),
  "--sticky-chrome-opacity": String(0.45 + displayOpacity.value * 0.55),
  "--sticky-line-opacity": String(0.04 + displayOpacity.value * 0.1),
  "--sticky-image-max-height": `${imageMaxHeight.value}px`,
}));
watch(appearanceOpen, (open) => {
  if (open) previewOwner.value = current.label;
  else if (previewOwner.value === current.label) previewOwner.value = undefined;
  void emit(STICKY_OPACITY_PREVIEW, { label: current.label, open }).catch((cause) => console.warn("便签透明度预览同步失败", cause));
});
const imageBroken = ref(false);
const imageSource = computed(() => payload.value?.imageSource || payload.value?.entry.imageInfo?.thumbnail);
const title = computed(() => payload.value?.entry.content.split(/\r?\n/, 1)[0]?.slice(0, 120) || "便签");
const unlisteners: UnlistenFn[] = [];
const shell = ref<HTMLElement>();
const content = ref<HTMLElement>();
const contentBody = ref<HTMLElement>();
const contentScrollable = ref(false);
const imageMaxHeight = ref(240);
let contentObserver: ResizeObserver | undefined;
let fitFrame: number | undefined;
let fitting = false;
let fitAgain = false;
let saveTimer: ReturnType<typeof setTimeout> | undefined;
let disposed = false;
let shown = false;

async function persistLayout(): Promise<void> {
  if (!payload.value) return;
  const [position, alwaysOnTop] = await Promise.all([current.outerPosition(), current.isAlwaysOnTop()]);
  await emitTo("main", STICKY_ACTION, {
    label: current.label, action: "layout",
    layout: { x: position.x, y: position.y, alwaysOnTop },
  } satisfies StickyAction);
}
function scheduleSave(): void {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => { void persistLayout().catch(() => {}); }, 150);
}

async function updateOpacity(): Promise<void> {
  try {
    localStorage.setItem(OPACITY_STORAGE_KEY, String(opacity.value));
    // Broadcast directly to all note windows, even while the main is hidden.
    await emit(STICKY_OPACITY, opacity.value);
  } catch (cause) { error.value = `保存透明度失败：${String(cause)}`; }
}

// Measure intrinsic content in a detached-from-layout copy, then constrain it
// to the current monitor. The real viewport remains scrollable for long notes.
async function fitContent(): Promise<void> {
  await nextTick();
  if (disposed || !payload.value || !shell.value || !content.value || !contentBody.value) return;
  const [monitor, position] = await Promise.all([currentMonitor(), current.outerPosition()]);
  if (disposed) return;
  const scale = monitor?.scaleFactor ?? await current.scaleFactor();
  const minWidth = appearanceOpen.value ? 260 : 200;
  const minHeight = appearanceOpen.value ? 200 : 64;
  const maxWidth = Math.max(minWidth, Math.min(420, monitor ? monitor.workArea.size.width / scale - 24 : 420));
  const maxHeight = Math.max(minHeight, Math.min(360, monitor ? monitor.workArea.size.height / scale - 24 : 360));
  const contentStyle = getComputedStyle(content.value);
  const paddingX = parseFloat(contentStyle.paddingLeft) + parseFloat(contentStyle.paddingRight);
  const paddingY = parseFloat(contentStyle.paddingTop) + parseFloat(contentStyle.paddingBottom);
  let chromeHeight = 2;
  for (const child of Array.from(shell.value.children)) {
    if (child === content.value) continue;
    const style = getComputedStyle(child);
    if (style.position === "absolute" || style.position === "fixed") continue;
    chromeHeight += child.getBoundingClientRect().height + parseFloat(style.marginTop) + parseFloat(style.marginBottom);
  }
  imageMaxHeight.value = Math.max(40, maxHeight - chromeHeight - paddingY);
  const probe = contentBody.value.cloneNode(true) as HTMLElement;
  const bodyStyle = getComputedStyle(contentBody.value);
  Object.assign(probe.style, {
    position: "fixed", left: "-10000px", top: "0", visibility: "hidden", pointerEvents: "none",
    width: "max-content", maxWidth: `${maxWidth - paddingX - 2}px`,
    fontFamily: bodyStyle.fontFamily, fontSize: bodyStyle.fontSize,
    fontWeight: bodyStyle.fontWeight, lineHeight: bodyStyle.lineHeight,
    letterSpacing: bodyStyle.letterSpacing,
  });
  const originalImages = contentBody.value.querySelectorAll("img");
  for (const [index, image] of probe.querySelectorAll("img").entries()) {
    image.style.maxWidth = `${maxWidth - paddingX - 2}px`;
    image.style.maxHeight = `${imageMaxHeight.value}px`;
    const original = originalImages[index];
    if (original?.naturalWidth && original.naturalHeight) {
      const ratio = Math.min(1, (maxWidth - paddingX - 2) / original.naturalWidth, imageMaxHeight.value / original.naturalHeight);
      image.style.width = `${original.naturalWidth * ratio}px`;
      image.style.height = `${original.naturalHeight * ratio}px`;
    }
  }
  document.body.append(probe);
  let width: number;
  let height: number;
  try {
    width = Math.ceil(Math.min(maxWidth, Math.max(minWidth, probe.getBoundingClientRect().width + paddingX + 2)));
    // Leave a small measurement margin for physical-pixel rounding at HiDPI.
    // Otherwise a final character can wrap after resizing and introduce a bar.
    const requiredHeight = () => {
      probe.style.width = `${width - paddingX - 4}px`;
      return Math.ceil(probe.getBoundingClientRect().height + paddingY + chromeHeight + 2);
    };
    let naturalHeight = requiredHeight();
    // Use available width before falling back to scrolling at the size cap.
    if (naturalHeight > maxHeight && width < maxWidth) {
      width = Math.ceil(maxWidth);
      naturalHeight = requiredHeight();
    }
    height = Math.min(maxHeight, Math.max(minHeight, naturalHeight));
    contentScrollable.value = naturalHeight > maxHeight;
  } finally { probe.remove(); }
  if (Math.abs(window.innerWidth - width) > 1 || Math.abs(window.innerHeight - height) > 1) {
    await current.setSize(new LogicalSize(width, height));
  }
  if (disposed || !monitor) return;
  const area = monitor.workArea;
  const x = Math.round(Math.max(area.position.x, Math.min(position.x, area.position.x + area.size.width - width * scale)));
  const y = Math.round(Math.max(area.position.y, Math.min(position.y, area.position.y + area.size.height - height * scale)));
  if (position.x !== x || position.y !== y) await current.setPosition(new PhysicalPosition(x, y));
}

function scheduleFit(): void {
  if (disposed || fitFrame !== undefined) return;
  if (fitting) { fitAgain = true; return; }
  fitFrame = requestAnimationFrame(() => {
    fitFrame = undefined;
    fitting = true;
    void fitContent().catch((cause) => { if (!disposed) console.warn("便签自动调整大小失败", cause); }).finally(() => {
      fitting = false;
      if (fitAgain) { fitAgain = false; scheduleFit(); }
    });
  });
}

watch([payload, error, appearanceOpen], scheduleFit, { flush: "post" });

async function toggleOnTop(): Promise<void> {
  try {
    await invoke("native_sticky_set_on_top", { onTop: !onTop.value });
    onTop.value = !onTop.value;
    await persistLayout();
  } catch (cause) { error.value = String(cause); }
}

async function forwardRightClick(event: MouseEvent): Promise<void> {
  event.preventDefault();
  event.stopPropagation();
  try { await invoke("native_sticky_forward_right_click"); }
  catch (cause) { error.value = `右键转发失败：${String(cause)}`; }
}

async function finishEdit(): Promise<void> {
  try { await invoke("native_sticky_finish_edit"); }
  catch (cause) { error.value = String(cause); }
}

async function action(value: StickyAction["action"]): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  if (value === "image") imageBroken.value = false;
  try { await emitTo("main", STICKY_ACTION, { label: current.label, action: value } satisfies StickyAction); }
  catch (cause) { busy.value = false; error.value = String(cause); }
}

onMounted(async () => {
  const add = (unlisten: UnlistenFn) => { if (disposed) unlisten(); else unlisteners.push(unlisten); };
  try {
    add(await listen<number>(STICKY_OPACITY, ({ payload: value }) => {
      if (Number.isFinite(value) && value >= 0 && value <= 100) opacity.value = value;
    }, { target: current.label }));
    add(await listen<{ label: string; open: boolean }>(STICKY_OPACITY_PREVIEW, ({ payload: preview }) => {
      if (preview.open) {
        previewOwner.value = preview.label;
        if (preview.label !== current.label) appearanceOpen.value = false;
      } else if (previewOwner.value === preview.label) previewOwner.value = undefined;
    }, { target: current.label }));
    // Request content before optional native configuration can delay or fail.
    add(await listen<StickyPayload>(STICKY_CONTENT, ({ payload: value }) => {
      if (value.imageSource !== payload.value?.imageSource) imageBroken.value = false;
      payload.value = value;
      void current.setTitle(`便签 · ${title.value}`).catch(() => {});
      if (!shown) {
        shown = true;
        void fitContent().then(() => { if (!disposed) return current.show(); }).catch((cause) => {
          shown = false;
          console.warn("便签显示失败", cause);
        });
      }
    }, { target: current.label }));
    await emitTo("main", STICKY_ACTION, { label: current.label, action: "ready" } satisfies StickyAction);
    await current.setResizable(false);
    await current.setMaximizable(false);
    await current.setMinimizable(false);
    await current.setMinSize(new LogicalSize(200, 64));
    contentObserver = new ResizeObserver(scheduleFit);
    if (contentBody.value) contentObserver.observe(contentBody.value);
    onTop.value = await current.isAlwaysOnTop();
    add(await listen<string>(STICKY_RESULT, ({ payload: message }) => { busy.value = false; error.value = message; }, { target: current.label }));
    add(await listen<StickyInputMode>(STICKY_INPUT, ({ payload: mode }) => {
      inputMode.value = mode;
      if (!mode.interactive) appearanceOpen.value = false;
    }, { target: current.label }));
    await invoke("native_sticky_input_mode");
    add(await current.onMoved(() => { scheduleSave(); scheduleFit(); }));
    add(await current.onResized(scheduleSave));
    add(await current.onCloseRequested((event) => { event.preventDefault(); void action("unpin"); }));
  } catch (cause) { error.value = `便签初始化失败：${String(cause)}`; }
});
onBeforeUnmount(() => {
  disposed = true;
  if (appearanceOpen.value) void emit(STICKY_OPACITY_PREVIEW, { label: current.label, open: false }).catch(() => {});
  if (saveTimer) clearTimeout(saveTimer);
  if (fitFrame !== undefined) cancelAnimationFrame(fitFrame);
  contentObserver?.disconnect();
  for (const unlisten of unlisteners) unlisten();
});
</script>

<template>
  <main @contextmenu.capture="forwardRightClick" class="sticky-window" :class="{ 'click-through': clickThrough }" :style="appearanceStyle">
    <div ref="shell" class="sticky-shell">
    <header class="sticky-header">
      <div class="sticky-drag" data-tauri-drag-region aria-label="拖动便签">
        <span v-if="payload?.entry.kind !== 'text'" data-tauri-drag-region :title="title">{{ title }}</span>
      </div>
      <div class="sticky-toolbar">
      <span v-if="clickThrough" class="sticky-passive-label"><MousePointer2 :size="12" aria-hidden="true" />{{ inputMode.altSupported ? '按住 Alt 操作' : '托盘编辑' }}</span>
      <div class="sticky-actions" :class="{ concealed: clickThrough }" :inert="clickThrough">
        <button v-if="inputMode.manualEdit" class="sticky-icon" aria-label="完成编辑" title="完成编辑，恢复穿透" @click="finishEdit"><Check :size="14" aria-hidden="true" /></button>
        <button v-if="payload?.entry.kind !== 'files'" class="sticky-icon" aria-label="复制" title="复制" :disabled="busy || !payload" @click="action('copy')"><Copy :size="14" aria-hidden="true" /></button>
        <button v-if="payload && payload.entry.kind !== 'text'" class="sticky-icon" aria-label="另存为" title="另存为" :disabled="busy" @click="action('save')"><Download :size="14" aria-hidden="true" /></button>
        <button class="sticky-icon" aria-label="调整便签透明度" title="调整所有便签的透明度" :aria-expanded="appearanceOpen" aria-controls="sticky-appearance" @click="appearanceOpen = !appearanceOpen"><SlidersHorizontal :size="14" aria-hidden="true" /></button>
        <button class="sticky-icon" :aria-label="onTop ? '取消置顶' : '置顶'" :title="onTop ? '取消置顶' : '置顶'" :aria-pressed="onTop" @click="toggleOnTop"><Pin :size="14" aria-hidden="true" /></button>
        <button class="sticky-icon" aria-label="关闭并取消便签" title="关闭并取消便签，同步到其他设备" :disabled="busy || !payload" @click="action('unpin')"><X :size="16" aria-hidden="true" /></button>
      </div>
      </div>
    </header>
    <section ref="content" class="sticky-content" :class="{ scrollable: contentScrollable }" aria-label="便签内容">
      <div ref="contentBody" class="sticky-content-body">
      <p v-if="!payload" role="status">正在加载便签…</p>
      <template v-else>
        <div v-if="payload.entry.kind === 'text'" class="sticky-text">{{ payload.entry.content }}</div>
        <template v-else-if="payload.entry.kind === 'image'">
          <img v-if="imageSource && !imageBroken" class="sticky-image" :src="imageSource" :alt="payload.entry.content" @load="scheduleFit" @error="imageBroken = true" />
          <p v-if="payload.imageError || imageBroken" class="sticky-status" role="status">{{ imageBroken ? '图片无法显示' : payload.imageError }} <button :disabled="busy" @click="action('image')">重试</button></p>
        </template>
        <StickyFiles v-else-if="payload.entry.fileInfo" :tree="payload.entry.fileInfo" />
      </template>
      </div>
    </section>
    <p v-if="error" class="sticky-error" role="alert">{{ error }}</p>
    </div>
    <div v-if="appearanceOpen && !clickThrough" id="sticky-appearance" class="sticky-appearance">
      <div class="sticky-appearance-heading">
        <label for="sticky-opacity">所有便签不透明度 <output for="sticky-opacity">{{ opacity }}%</output></label>
        <button class="sticky-icon" aria-label="关闭透明度设置" title="关闭透明度设置" @click="appearanceOpen = false"><X :size="14" aria-hidden="true" /></button>
      </div>
      <input id="sticky-opacity" v-model.number="opacity" type="range" min="0" max="100" step="1" @input="updateOpacity" @change="updateOpacity" />
      <p>背景按比例变化，标题和内容保留最低可见度；按住 Alt 操作时恢复清晰。</p>
    </div>
  </main>
</template>

<style scoped>
.sticky-window {
  --sticky-accent: #b8c6d9;
  --sticky-muted: #c0c9d6;
  --sticky-line: rgb(203 213 225 / var(--sticky-line-opacity, 0.14));
  position: relative; height: 100%;
}
.sticky-shell {
  position: relative; display: flex; flex-direction: column; height: 100%; overflow: hidden;
  color: #f8fafc; background: rgb(17 26 41 / var(--sticky-background-opacity, 0.78));
  border: 1px solid var(--sticky-line); border-radius: 12px; font-size: 14px;
}
.sticky-header { display: flex; flex: 0 0 30px; height: 30px; align-items: center; padding: 0 5px 0 10px; gap: 1px; user-select: none; opacity: var(--sticky-chrome-opacity, 1); }
.sticky-toolbar { position: relative; flex-shrink: 0; }
.sticky-actions { display: flex; align-items: center; gap: 1px; }
.sticky-actions.concealed { visibility: hidden; pointer-events: none; }
.sticky-drag { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; align-self: stretch; cursor: move; }
.sticky-drag > span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--sticky-muted); }
button { display: inline-flex; justify-content: center; align-items: center; gap: 6px; min-height: 30px; padding: 4px 7px; border: 0; border-radius: 6px; background: transparent; color: inherit; font-size: 12px; cursor: pointer; transition: background-color 150ms ease, color 150ms ease; }
button:hover:not(:disabled) { background: var(--sui-bg-hover); }
.sticky-icon { width: 28px; padding: 4px; flex-shrink: 0; color: var(--sticky-muted); }
.sticky-icon[aria-pressed="true"] { color: var(--sticky-accent); background: var(--sui-bg-hover); }
.sticky-content { flex: 1; min-height: 0; padding: 4px 10px 8px; overflow: hidden; scrollbar-width: thin; scrollbar-color: rgba(203, 213, 225, 0.3) transparent; opacity: var(--sticky-content-opacity, 1); }
.sticky-content.scrollable { overflow-y: auto; }
.sticky-text { white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.5; user-select: text; text-shadow: 0 1px 3px rgba(0, 0, 0, 0.45); }
.sticky-image { display: block; max-width: 100%; max-height: var(--sticky-image-max-height, 240px); width: auto; height: auto; object-fit: contain; margin: 0 auto; border-radius: 7px; }
.sticky-status { color: var(--sticky-muted); font-size: 12px; overflow-wrap: anywhere; }
.sticky-error { padding: 8px 12px; margin: 0; max-height: 80px; overflow: auto; color: var(--sui-error, #fca5a5); font-size: 12px; }
.sticky-appearance { position: absolute; z-index: 2; top: 30px; left: 8px; right: 8px; max-height: calc(100% - 38px); overflow: auto; padding: 10px 12px; border: 1px solid var(--sticky-line); border-radius: 8px; background: rgba(17, 26, 41, 0.98); box-shadow: 0 6px 18px rgba(0, 0, 0, 0.2); }
.sticky-appearance-heading { display: flex; align-items: center; gap: 6px; }
.sticky-appearance label { display: flex; flex: 1; justify-content: space-between; gap: 8px; color: var(--sticky-muted); font-size: 12px; }
.sticky-appearance output { color: var(--sticky-accent); font-variant-numeric: tabular-nums; }
.sticky-appearance input { display: block; width: 100%; height: 24px; margin: 6px 0; accent-color: var(--sticky-accent); cursor: pointer; }
.sticky-appearance input:focus-visible { outline: 2px solid var(--sticky-accent); outline-offset: 2px; }
.sticky-appearance p { color: var(--sticky-muted); font-size: 12px; line-height: 1.6; margin: 0; }
.sticky-passive-label { position: absolute; inset: 0; display: flex; align-items: center; justify-content: flex-end; gap: 4px; white-space: nowrap; color: var(--sticky-muted); font-size: 11px; padding: 0 6px; }
@media (prefers-reduced-motion: reduce) { button { transition: none; } }
</style>

<style>
html.sticky-window-root,
html.sticky-window-root body,
html.sticky-window-root #app { background: transparent !important; }
</style>
