<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";

const props = defineProps<{ src: string; title: string; refreshSource: () => Promise<string> }>();
const video = ref<HTMLVideoElement>();
const source = ref(props.src);
const loading = ref(true);
const refreshing = ref(false);
const error = ref("");
let issuedAt = Date.now();
let disposed = false;
let resume: { time: number; playing: boolean } | undefined;

async function refresh(): Promise<void> {
  if (refreshing.value || disposed) return;
  const player = video.value;
  resume ??= { time: player?.currentTime ?? 0, playing: !!player && !player.paused };
  refreshing.value = true;
  loading.value = true;
  error.value = "";
  try {
    const url = await props.refreshSource();
    if (disposed) return;
    issuedAt = Date.now();
    source.value = url;
    // Retry even if the server issued the same URL within this millisecond.
    if (player) { player.src = url; player.load(); }
  } catch {
    if (!disposed) {
      error.value = "解析/播放失败";
      loading.value = false;
    }
  } finally {
    refreshing.value = false;
  }
}

function onError(): void {
  if (disposed || refreshing.value) return;
  // Access URLs last one hour. Refresh on a failed range request after expiry,
  // without interrupting an active connection or periodically resetting playback.
  if (video.value?.error?.code !== 3 && Date.now() - issuedAt >= 55 * 60 * 1000) {
    void refresh();
    return;
  }
  loading.value = false;
  error.value = "解析/播放失败";
}

function onMetadata(): void {
  loading.value = false;
  const player = video.value;
  if (!player || !resume) return;
  const previous = resume;
  resume = undefined;
  if (Number.isFinite(previous.time) && previous.time > 0) player.currentTime = previous.time;
  if (previous.playing) void player.play().catch(() => {});
}

onBeforeUnmount(() => {
  disposed = true;
  const player = video.value;
  if (!player) return;
  player.pause();
  player.removeAttribute("src");
  player.load();
});
</script>

<template>
  <div class="video-preview">
    <video
      ref="video"
      :src="source"
      :aria-label="`视频播放：${title}`"
      controls
      controlslist="nodownload"
      playsinline
      preload="metadata"
      @loadedmetadata="onMetadata"
      @canplay="loading = false"
      @playing="loading = false"
      @waiting="loading = true"
      @error="onError"
    >解析/播放失败</video>
    <div v-if="error" class="video-preview-status" role="alert">
      <p>{{ error }}</p>
      <SButton :disabled="refreshing" @click="refresh">重试</SButton>
    </div>
    <p v-else-if="loading" class="video-preview-status" role="status">{{ refreshing ? '正在重新连接视频…' : '正在缓冲视频…' }}</p>
  </div>
</template>

<style scoped>
.video-preview { padding: 16px; }
.video-preview video { display: block; width: 100%; max-height: min(600px, calc(100dvh - 240px)); min-height: 160px; border-radius: 8px; background: #000; color-scheme: dark; }
.video-preview-status { margin: 12px 0 0; color: var(--color-secondary-text); font-size: 13px; text-align: center; }
.video-preview-status p { margin: 0 0 12px; }
@media (max-width: 640px) { .video-preview { padding: 12px; } }
</style>
