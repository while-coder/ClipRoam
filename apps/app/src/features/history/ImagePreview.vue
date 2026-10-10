<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry } from "../../types";

const props = defineProps<{
  entry: LocalClipboardEntry;
  ensureLocalFiles: (entry: LocalClipboardEntry) => Promise<LocalClipboardEntry>;
}>();
const emit = defineEmits<{ close: [] }>();
const previewImage = ref<LocalClipboardEntry>();
const loading = ref(true);
const error = ref("");
let disposed = false;

function imageSource(entry: LocalClipboardEntry): string | undefined {
  const path = entry.summary.previewPath;
  return path ? convertFileSrc(path) : undefined;
}

onMounted(async () => {
  try {
    const entry = await props.ensureLocalFiles(props.entry);
    if (disposed) return;
    if (!imageSource(entry)) throw new Error("图片文件不可用");
    previewImage.value = entry;
  } catch (cause) {
    if (!disposed) error.value = `无法预览图片：${errorMessage(cause)}`;
  } finally {
    if (!disposed) loading.value = false;
  }
});
onBeforeUnmount(() => { disposed = true; resetPreviewGesture(); });

// 预览缩放/平移：desktop 滚轮缩放、左键拖拽平移；移动端双指捏合缩放、单指拖拽；
// 双击复位。全部走 transform，不产生滚动条。
const previewZoom = ref(1);
const previewPanX = ref(0);
const previewPanY = ref(0);
const PREVIEW_ZOOM_MIN = 0.25;
const PREVIEW_ZOOM_MAX = 6;
const previewPointers = new Map<number, { x: number; y: number; type: string }>();
let previewPanDrag: { id: number; startX: number; startY: number; panX: number; panY: number } | null = null;
let previewPinch: { distance: number; zoom: number } | null = null;
// stage/img 的布局尺寸：平移边界要用，手势开始与图片加载时读一次，之后纯算术。
const previewStageElement = ref<HTMLElement>();
const previewImageElement = ref<HTMLImageElement>();
let previewMetrics: { contentW: number; contentH: number; imgW: number; imgH: number } | null = null;
function clampPreviewZoom(value: number): number {
  return Math.min(PREVIEW_ZOOM_MAX, Math.max(PREVIEW_ZOOM_MIN, value));
}

function resetPreviewTransform(): void {
  previewZoom.value = 1;
  previewPanX.value = 0;
  previewPanY.value = 0;
}

/** 读取 stage 内容区与图片的布局尺寸，作为平移边界计算的基准。 */
function readPreviewMetrics(): void {
  const stage = previewStageElement.value;
  const image = previewImageElement.value;
  if (!stage || !image) {
    previewMetrics = null;
    return;
  }
  const padding = getComputedStyle(stage);
  previewMetrics = {
    contentW: stage.clientWidth - parseFloat(padding.paddingLeft) - parseFloat(padding.paddingRight),
    contentH: stage.clientHeight - parseFloat(padding.paddingTop) - parseFloat(padding.paddingBottom),
    imgW: image.offsetWidth,
    imgH: image.offsetHeight,
  };
}

/** 平移限制在「缩放后的图片不被拖出 stage 内容区」范围内；尺寸未知时不作限制。 */
function clampPreviewPan(): void {
  if (!previewMetrics) return;
  const limitX = Math.max(0, (previewMetrics.imgW * previewZoom.value - previewMetrics.contentW) / 2);
  const limitY = Math.max(0, (previewMetrics.imgH * previewZoom.value - previewMetrics.contentH) / 2);
  previewPanX.value = Math.min(limitX, Math.max(-limitX, previewPanX.value));
  previewPanY.value = Math.min(limitY, Math.max(-limitY, previewPanY.value));
}

function onPreviewWheel(event: WheelEvent): void {
  readPreviewMetrics();
  previewZoom.value = clampPreviewZoom(previewZoom.value * Math.exp(-event.deltaY * 0.002));
  clampPreviewPan();
}

function onPreviewImageLoad(): void {
  readPreviewMetrics();
  clampPreviewPan();
}

function previewPointersDistance(): number {
  const [first, second] = [...previewPointers.values()];
  return Math.hypot(first.x - second.x, first.y - second.y);
}

/** 仅剩一根指针且不在捏合时，把它设为新的平移基线（避免松指后跳位）。 */
function syncPreviewPanDrag(): void {
  if (previewPointers.size !== 1 || previewPinch) {
    previewPanDrag = null;
    return;
  }
  const [id, point] = [...previewPointers.entries()][0];
  previewPanDrag = { id, startX: point.x, startY: point.y, panX: previewPanX.value, panY: previewPanY.value };
}

function detachPreviewPointerListeners(): void {
  window.removeEventListener("pointermove", onPreviewPointerMove);
  window.removeEventListener("pointerup", onPreviewPointerUp);
  window.removeEventListener("pointercancel", onPreviewPointerUp);
}

/** 清掉手势状态与 window 监听；关闭预览与组件卸载时调用。 */
function resetPreviewGesture(): void {
  previewPointers.clear();
  previewPanDrag = null;
  previewPinch = null;
  detachPreviewPointerListeners();
}

function onPreviewPointerDown(event: PointerEvent): void {
  if (event.button !== 0) return;
  // 指针可能在窗口外松开、收不到 pointerup（鼠标 pointerId 恒定）：残留的幽灵
  // 记录会让本次手势被误判成双指捏合，表现为拖不动。同类型非 touch 只有一个活跃指针。
  if (event.pointerType !== "touch") {
    for (const [id, point] of [...previewPointers.entries()]) {
      if (point.type === event.pointerType) previewPointers.delete(id);
    }
    if (previewPointers.size < 2) previewPinch = null;
  }
  previewPointers.set(event.pointerId, { x: event.clientX, y: event.clientY, type: event.pointerType });
  if (previewPointers.size === 2) {
    previewPinch = { distance: previewPointersDistance(), zoom: previewZoom.value };
    previewPanDrag = null;
  } else {
    syncPreviewPanDrag();
  }
  // move/up 挂 window：拖出 stage 或窗口边缘都不丢事件（比 setPointerCapture 稳）。
  readPreviewMetrics();
  window.addEventListener("pointermove", onPreviewPointerMove);
  window.addEventListener("pointerup", onPreviewPointerUp);
  window.addEventListener("pointercancel", onPreviewPointerUp);
}

function onPreviewPointerMove(event: PointerEvent): void {
  if (!previewPointers.has(event.pointerId)) return;
  previewPointers.set(event.pointerId, { x: event.clientX, y: event.clientY, type: event.pointerType });
  if (previewPinch && previewPointers.size >= 2) {
    previewZoom.value = clampPreviewZoom(previewPinch.zoom * (previewPointersDistance() / previewPinch.distance));
    clampPreviewPan();
  } else if (previewPanDrag?.id === event.pointerId) {
    previewPanX.value = previewPanDrag.panX + event.clientX - previewPanDrag.startX;
    previewPanY.value = previewPanDrag.panY + event.clientY - previewPanDrag.startY;
    clampPreviewPan();
  }
}

function onPreviewPointerUp(event: PointerEvent): void {
  previewPointers.delete(event.pointerId);
  if (previewPointers.size < 2) previewPinch = null;
  if (previewPanDrag?.id === event.pointerId) previewPanDrag = null;
  if (previewPointers.size > 0) {
    syncPreviewPanDrag();
    return;
  }
  detachPreviewPointerListeners();
}

</script>

<template>
    <SModal
      :show="true"
      :title="entry.content"
      width="980px"
      @update:show="(value: boolean) => { if (!value) emit('close') }"
    >
      <div v-if="loading" class="preview-status" role="status">
        <span class="s-spinner" aria-hidden="true" />正在加载图片…
      </div>
      <div v-else-if="error" class="preview-status" role="alert">{{ error }}</div>
      <div
        v-else-if="previewImage"
        ref="previewStageElement"
        class="image-preview-stage"
        @wheel.prevent="onPreviewWheel"
        @pointerdown="onPreviewPointerDown"
        @dblclick="resetPreviewTransform"
      >
        <img
          ref="previewImageElement"
          :src="imageSource(previewImage)"
          :alt="previewImage.content"
          :style="{ transform: `translate(${previewPanX}px, ${previewPanY}px) scale(${previewZoom})` }"
          draggable="false"
          @load="onPreviewImageLoad"
          @error="error = '图片加载失败'"
        />
      </div>
    </SModal>
</template>

<style scoped>
.preview-status { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 160px; color: var(--color-secondary-text); }
.image-preview-stage {
  display: grid;
  place-items: center;
  min-height: 0;
  /* 预览已是 SModal 内容：高度自算（视口 - 标题栏/内边距），img max-height:100% 才有约束。 */
  height: min(640px, calc(100vh - 200px));
  padding: 20px;
  /* 缩放/平移全走 transform：溢出即裁剪，不给 scale 生成滚动条。 */
  overflow: hidden;
  /* 手势交给 pointer 处理（移动端双指捏合/单指拖），不触发页面自身的缩放滚动。 */
  touch-action: none;
  cursor: grab;
  background: radial-gradient(circle at center, #17233c, #050a14 74%);
}
.image-preview-stage:active { cursor: grabbing; }
.image-preview-stage img { display: block; max-width: 100%; max-height: 100%; object-fit: contain; user-select: none; }
</style>
