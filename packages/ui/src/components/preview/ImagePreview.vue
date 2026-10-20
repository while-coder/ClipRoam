<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";

defineProps<{ src: string; title: string }>();
const error = ref("");
const previewZoom = ref(1);
const previewPanX = ref(0);
const previewPanY = ref(0);
const previewImageSize = ref({ width: 0, height: 0 });
const PREVIEW_ZOOM_MIN = 0.25;
const PREVIEW_ZOOM_MAX = 6;
type PreviewPoint = { x: number; y: number };
const previewPointers = new Map<number, PreviewPoint>();
const previewStageElement = ref<HTMLElement>();
const previewImageElement = ref<HTMLImageElement>();
let previewMetrics: { contentW: number; contentH: number; centerX: number; centerY: number } | null = null;
let previewResizeObserver: ResizeObserver | undefined;

onMounted(() => {
  previewResizeObserver = new ResizeObserver(() => {
    readPreviewMetrics();
    clampPreviewPan();
  });
  if (previewStageElement.value) previewResizeObserver.observe(previewStageElement.value);
});
onBeforeUnmount(() => {
  resetPreviewGesture();
  previewResizeObserver?.disconnect();
});

function resetPreviewTransform(): void {
  resetPreviewGesture();
  previewZoom.value = 1;
  previewPanX.value = 0;
  previewPanY.value = 0;
  readPreviewMetrics();
}

/** 从原始尺寸等比适配，避免 Grid 固有尺寸和 object-fit 的留白影响显示与拖拽边界。 */
function readPreviewMetrics(): void {
  const stage = previewStageElement.value;
  const image = previewImageElement.value;
  if (!stage || !image) {
    previewMetrics = null;
    return;
  }
  const padding = getComputedStyle(stage);
  const left = parseFloat(padding.paddingLeft);
  const top = parseFloat(padding.paddingTop);
  const contentW = Math.max(0, stage.clientWidth - left - parseFloat(padding.paddingRight));
  const contentH = Math.max(0, stage.clientHeight - top - parseFloat(padding.paddingBottom));
  const rect = stage.getBoundingClientRect();
  previewMetrics = {
    contentW, contentH,
    centerX: rect.left + stage.clientLeft + left + contentW / 2,
    centerY: rect.top + stage.clientTop + top + contentH / 2,
  };
  if (image.naturalWidth > 0 && image.naturalHeight > 0) {
    const fit = Math.min(1, contentW / image.naturalWidth, contentH / image.naturalHeight);
    previewImageSize.value = { width: image.naturalWidth * fit, height: image.naturalHeight * fit };
  }
}

function clampPreviewPan(): void {
  if (!previewMetrics) return;
  const limitX = Math.max(0, (previewImageSize.value.width * previewZoom.value - previewMetrics.contentW) / 2);
  const limitY = Math.max(0, (previewImageSize.value.height * previewZoom.value - previewMetrics.contentH) / 2);
  previewPanX.value = Math.min(limitX, Math.max(-limitX, previewPanX.value));
  previewPanY.value = Math.min(limitY, Math.max(-limitY, previewPanY.value));
}

/** 将旧锚点下的图片内容移到新锚点；坐标以预览内容区中心为原点。 */
function zoomPreviewAt(zoom: number, fromX: number, fromY: number, toX = fromX, toY = fromY): void {
  if (!Number.isFinite(zoom)) return;
  const next = Math.min(PREVIEW_ZOOM_MAX, Math.max(PREVIEW_ZOOM_MIN, zoom));
  const ratio = next / previewZoom.value;
  previewPanX.value = toX - (fromX - previewPanX.value) * ratio;
  previewPanY.value = toY - (fromY - previewPanY.value) * ratio;
  previewZoom.value = next;
  clampPreviewPan();
}

function onPreviewWheel(event: WheelEvent): void {
  readPreviewMetrics();
  if (!previewMetrics) return;
  // WheelEvent 的行/页单位不能直接当像素使用。
  const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? previewMetrics.contentH : 1;
  const factor = Math.exp(Math.max(-20, Math.min(20, -event.deltaY * unit * 0.002)));
  zoomPreviewAt(previewZoom.value * factor,
    event.clientX - previewMetrics.centerX, event.clientY - previewMetrics.centerY);
}

function onPreviewImageLoad(): void {
  resetPreviewTransform();
}

function previewPinchPoints(): { distance: number; x: number; y: number } | undefined {
  const [first, second] = previewPointers.values();
  if (!first || !second) return;
  return {
    distance: Math.hypot(first.x - second.x, first.y - second.y),
    x: (first.x + second.x) / 2,
    y: (first.y + second.y) / 2,
  };
}

function detachPreviewPointerListeners(): void {
  window.removeEventListener("pointermove", onPreviewPointerMove);
  window.removeEventListener("pointerup", onPreviewPointerUp);
  window.removeEventListener("pointercancel", onPreviewPointerUp);
  window.removeEventListener("blur", resetPreviewGesture);
}

function releasePreviewPointer(id: number): void {
  const stage = previewStageElement.value;
  if (stage?.hasPointerCapture(id)) stage.releasePointerCapture(id);
}

function resetPreviewGesture(): void {
  const ids = [...previewPointers.keys()];
  previewPointers.clear();
  for (const id of ids) releasePreviewPointer(id);
  detachPreviewPointerListeners();
}

function onPreviewPointerDown(event: PointerEvent): void {
  if (event.button !== 0) return;
  if (event.pointerType !== "touch") resetPreviewGesture();
  readPreviewMetrics();
  previewPointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  previewStageElement.value?.setPointerCapture(event.pointerId);
  window.addEventListener("pointermove", onPreviewPointerMove);
  window.addEventListener("pointerup", onPreviewPointerUp);
  window.addEventListener("pointercancel", onPreviewPointerUp);
  window.addEventListener("blur", resetPreviewGesture);
}

function onPreviewPointerMove(event: PointerEvent): void {
  const previous = previewPointers.get(event.pointerId);
  if (!previous) return;
  if (event.pointerType !== "touch" && (event.buttons & 1) === 0) {
    onPreviewPointerUp(event);
    return;
  }
  const before = previewPinchPoints();
  previewPointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  const after = previewPinchPoints();
  if (before && after && previewMetrics) {
    // 逐帧使用当前两指作基线，换指和到达缩放边界后均可立即反向操作。
    const ratio = before.distance > 0 && after.distance > 0 ? after.distance / before.distance : 1;
    zoomPreviewAt(previewZoom.value * ratio,
      before.x - previewMetrics.centerX, before.y - previewMetrics.centerY,
      after.x - previewMetrics.centerX, after.y - previewMetrics.centerY);
  } else if (previewPointers.size === 1) {
    // 使用相邻事件的位移，边界外的距离不累积成反向拖动的死区。
    previewPanX.value += event.clientX - previous.x;
    previewPanY.value += event.clientY - previous.y;
    clampPreviewPan();
  }
}

function onPreviewPointerUp(event: PointerEvent): void {
  if (!previewPointers.delete(event.pointerId)) return;
  releasePreviewPointer(event.pointerId);
  if (previewPointers.size === 0) detachPreviewPointerListeners();
}
</script>

<template>
  <div v-if="error" class="preview-status" role="alert">{{ error }}</div>
  <div
    v-else
    ref="previewStageElement"
    class="image-preview-stage"
    @wheel.prevent="onPreviewWheel"
    @pointerdown="onPreviewPointerDown"
    @lostpointercapture="onPreviewPointerUp"
    @dblclick="resetPreviewTransform"
  >
    <img
      ref="previewImageElement"
      :src="src"
      :alt="title"
      :style="{
        width: `${previewImageSize.width}px`, height: `${previewImageSize.height}px`,
        marginLeft: `${-previewImageSize.width / 2}px`, marginTop: `${-previewImageSize.height / 2}px`,
        transform: `translate(${previewPanX}px, ${previewPanY}px) scale(${previewZoom})`,
      }"
      draggable="false"
      @load="onPreviewImageLoad"
      @error="error = '图片加载失败'"
    />
  </div>
</template>

<style scoped>
.preview-status { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 160px; color: var(--color-secondary-text); }
.image-preview-stage {
  position: relative;
  /* 随弹窗 body 收缩，确保尺寸测量和拖拽边界对应实际可见区域。 */
  flex: 1 1 auto;
  min-height: 0;
  height: min(640px, calc(100vh - 200px));
  height: min(640px, calc(100dvh - 200px));
  padding: 20px;
  /* 缩放/平移全走 transform：溢出即裁剪，不给 scale 生成滚动条。 */
  overflow: hidden;
  /* 手势交给 pointer 处理（移动端双指捏合/单指拖），不触发页面自身的缩放滚动。 */
  touch-action: none;
  cursor: grab;
  background: radial-gradient(circle at center, #17233c, #050a14 74%);
}
.image-preview-stage:active { cursor: grabbing; }
.image-preview-stage img { position: absolute; left: 50%; top: 50%; display: block; max-width: none; max-height: none; user-select: none; }
</style>
