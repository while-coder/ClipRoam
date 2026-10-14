<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'
import DOMPurify from 'dompurify'
import { marked } from 'marked'
import { useTauriUpdater } from './useTauriUpdater'

const props = withDefaults(defineProps<{
  autoCheck?: boolean
  autoCheckDelayMs?: number
}>(), {
  autoCheck: true,
  autoCheckDelayMs: 0,
})

const updater = useTauriUpdater()
const updateStatus = computed(() => updater.updateStatus.value)
const updateVisible = computed(
  () => updateStatus.value !== 'idle' && updateStatus.value !== 'checking' && updateStatus.value !== 'latest',
)
const updateVersion = computed(() => updater.updateVersion.value)
const updateNotes = computed(() => updater.updateNotes.value)
const updateDownloaded = computed(() => updater.updateDownloaded.value)
const updateTotal = computed(() => updater.updateTotal.value)
const updateProgressPercent = computed(() => updater.updateProgressPercent.value)
const updateMsg = computed(() => updater.updateMsg.value)
const updateErrorText = computed(() => {
  if (!updater.runningInTauri) return '浏览器预览环境不支持应用内更新。'
  if (!updater.updaterSupported) return '当前平台暂不支持应用内更新。'
  return updateMsg.value || '检查更新失败。'
})
const hasPendingUpdate = computed(() => Boolean(updater.pendingUpdate.value))
const renderedUpdateNotes = computed(() => renderMarkdown(updateNotes.value))
const updateDate = computed(() => formatUpdateDate(updater.pendingUpdate.value?.date))

function formatUpdateDate(value?: string) {
  if (!value) return ''
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  }).format(date)
}

function renderMarkdown(value: string) {
  const html = marked.parse(value, { async: false }) as string
  return DOMPurify.sanitize(html)
}

onMounted(() => {
  if (props.autoCheck) updater.scheduleAutoCheck(props.autoCheckDelayMs)
})

onBeforeUnmount(() => {
  updater.dispose()
})

async function dismiss() {
  updater.updateStatus.value = 'idle'
  updater.updateMsg.value = ''
  await updater.closePendingUpdate()
}
</script>

<template>
  <Teleport to="body">
    <div v-if="updateVisible" class="tuk-updater-mask">
      <div class="tuk-updater-card">
        <template v-if="updateStatus === 'available'">
          <h3>发现新版本 {{ updateVersion }}</h3>
          <p v-if="updateDate" class="tuk-updater-date">发布日期：{{ updateDate }}</p>
          <div v-if="updateNotes" class="tuk-updater-notes" v-html="renderedUpdateNotes"></div>
          <div class="tuk-updater-row">
            <button class="tuk-updater-btn secondary" type="button" @click="dismiss">稍后</button>
            <button class="tuk-updater-btn primary" type="button" @click="updater.downloadAndInstallUpdate">现在更新</button>
          </div>
        </template>

        <template v-else-if="updateStatus === 'downloading'">
          <h3>正在下载更新...</h3>
          <div class="tuk-updater-bar">
            <div class="tuk-updater-bar-fill" :style="{ width: updateProgressPercent + '%' }" />
          </div>
          <p class="tuk-updater-muted">
            {{ updateProgressPercent }}%
            <span v-if="updateTotal">
              ({{ Math.round(updateDownloaded / 1024) }} / {{ Math.round(updateTotal / 1024) }} KB)
            </span>
          </p>
        </template>

        <template v-else-if="updateStatus === 'installed'">
          <h3>更新已就绪</h3>
          <p>需要重启应用以完成更新。</p>
          <div class="tuk-updater-row">
            <button class="tuk-updater-btn secondary" type="button" @click="dismiss">稍后重启</button>
            <button class="tuk-updater-btn primary" type="button" @click="updater.relaunchApp">立即重启</button>
          </div>
        </template>

        <template v-else-if="updateStatus === 'error'">
          <h3>更新失败</h3>
          <p class="tuk-updater-error">{{ updateErrorText }}</p>
          <div class="tuk-updater-row">
            <button class="tuk-updater-btn secondary" type="button" @click="dismiss">关闭</button>
            <button
              v-if="hasPendingUpdate"
              class="tuk-updater-btn primary"
              type="button"
              @click="updater.downloadAndInstallUpdate"
            >
              重试
            </button>
          </div>
        </template>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.tuk-updater-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
  font-size: 14px;
}

.tuk-updater-card {
  width: min(480px, calc(100vw - 32px));
  min-width: 320px;
  padding: 20px;
  border: 1px solid var(--border, transparent);
  border-radius: var(--radius, 8px);
  background: var(--card, #fff);
  color: var(--fg, #1a1a1a);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.2);
}

.tuk-updater-card h3 {
  margin: 0 0 12px;
  font-size: 16px;
}

.tuk-updater-card p {
  margin: 0;
}

.tuk-updater-date {
  margin: -6px 0 12px !important;
  color: var(--fg-muted, #888);
  font-size: 12px;
}

.tuk-updater-notes {
  max-height: 200px;
  padding: 8px 12px;
  overflow: auto;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.1);
  font-size: 13px;
  line-height: 1.55;
}

.tuk-updater-notes :deep(*) {
  box-sizing: border-box;
}

.tuk-updater-notes :deep(h1),
.tuk-updater-notes :deep(h2),
.tuk-updater-notes :deep(h3),
.tuk-updater-notes :deep(h4),
.tuk-updater-notes :deep(h5),
.tuk-updater-notes :deep(h6) {
  margin: 12px 0 6px;
  color: inherit;
  font-weight: 700;
  line-height: 1.35;
}

.tuk-updater-notes :deep(h1:first-child),
.tuk-updater-notes :deep(h2:first-child),
.tuk-updater-notes :deep(h3:first-child),
.tuk-updater-notes :deep(h4:first-child),
.tuk-updater-notes :deep(h5:first-child),
.tuk-updater-notes :deep(h6:first-child) {
  margin-top: 0;
}

.tuk-updater-notes :deep(h1),
.tuk-updater-notes :deep(h2) {
  font-size: 15px;
}

.tuk-updater-notes :deep(h3),
.tuk-updater-notes :deep(h4),
.tuk-updater-notes :deep(h5),
.tuk-updater-notes :deep(h6) {
  font-size: 14px;
}

.tuk-updater-notes :deep(p),
.tuk-updater-notes :deep(ul),
.tuk-updater-notes :deep(ol) {
  margin: 0 0 8px;
}

.tuk-updater-notes :deep(:last-child) {
  margin-bottom: 0;
}

.tuk-updater-notes :deep(ul),
.tuk-updater-notes :deep(ol) {
  padding-left: 1.35em;
}

.tuk-updater-notes :deep(li + li) {
  margin-top: 4px;
}

.tuk-updater-notes :deep(code) {
  padding: 1px 4px;
  border-radius: 3px;
  background: rgba(127, 127, 127, 0.18);
  font-family: ui-monospace, SFMono-Regular, Consolas, 'Liberation Mono', Menlo, monospace;
  font-size: 0.92em;
}

.tuk-updater-notes :deep(a) {
  color: var(--primary, #2d6cdf);
  text-decoration: none;
}

.tuk-updater-notes :deep(a:hover) {
  text-decoration: underline;
}

.tuk-updater-row {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}

.tuk-updater-btn {
  padding: 6px 14px;
  border: 1px solid transparent;
  border-radius: 4px;
  cursor: pointer;
  font-size: 13px;
}

.tuk-updater-btn.primary {
  background: var(--primary, #2d6cdf);
  color: #fff;
}

.tuk-updater-btn.primary:hover {
  background: var(--primary-hover, #1f56b8);
}

.tuk-updater-btn.secondary {
  border-color: var(--border, rgba(127, 127, 127, 0.3));
  background: transparent;
  color: inherit;
}

.tuk-updater-btn.secondary:hover {
  background: rgba(127, 127, 127, 0.1);
}

.tuk-updater-bar {
  height: 8px;
  margin: 12px 0;
  overflow: hidden;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.2);
}

.tuk-updater-bar-fill {
  height: 100%;
  background: var(--primary, #2d6cdf);
  transition: width 0.2s;
}

.tuk-updater-muted {
  color: var(--fg-muted, #888);
  font-size: 12px;
}

.tuk-updater-error {
  color: var(--danger, #d14343);
  font-size: 13px;
  word-break: break-all;
}
</style>
