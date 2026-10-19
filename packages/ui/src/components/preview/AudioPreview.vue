<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import { Music } from "lucide-vue-next";

const props = defineProps<{ src: string; title: string }>();
const audio = ref<HTMLAudioElement>();
const loading = ref(true);
const error = ref("");

watch(() => props.src, () => { loading.value = true; error.value = ""; });

function onError(): void {
  loading.value = false;
  error.value = audio.value?.error?.code === 3
    ? "音频解码失败，文件可能损坏或当前系统不支持此编码。"
    : "无法播放此音频，当前系统可能不支持此格式或文件内容不可用。";
}

onBeforeUnmount(() => {
  const player = audio.value;
  if (!player) return;
  player.pause();
  player.removeAttribute("src");
  player.load();
});
</script>

<template>
  <div class="audio-preview">
    <div class="audio-preview-card">
      <Music :size="40" class="audio-preview-icon" aria-hidden="true" />
      <p class="audio-preview-title">{{ title }}</p>
      <audio
        ref="audio"
        :src="src"
        :aria-label="`音频播放：${title}`"
        controls
        controlslist="nodownload"
        preload="metadata"
        @loadedmetadata="loading = false"
        @canplay="loading = false"
        @error="onError"
      >当前系统不支持音频播放。</audio>
      <p v-if="error" class="audio-preview-status" role="alert">{{ error }}</p>
      <p v-else-if="loading" class="audio-preview-status" role="status">正在读取音频…</p>
    </div>
  </div>
</template>

<style scoped>
.audio-preview { display: grid; place-items: center; min-height: 260px; padding: 24px; }
.audio-preview-card { display: grid; justify-items: center; gap: 20px; width: 100%; max-width: 560px; padding: 24px; border: 1px solid var(--sui-border-strong); border-radius: 8px; background: var(--sui-bg-hover); }
.audio-preview-icon { color: var(--sui-primary); }
.audio-preview-title { max-width: 100%; margin: 0; overflow-wrap: anywhere; text-align: center; color: var(--color-foreground); font-size: 15px; }
.audio-preview-card audio { display: block; width: 100%; min-width: 0; color-scheme: dark; }
.audio-preview-status { margin: 0; color: var(--color-secondary-text); font-size: 13px; text-align: center; }
@media (max-width: 640px) {
  .audio-preview { padding: 12px; }
  .audio-preview-card { padding: 20px 12px; }
}
</style>
