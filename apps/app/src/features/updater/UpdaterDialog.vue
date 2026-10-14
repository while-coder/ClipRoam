<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'
import SModal from '@qingfeng346/ui-kit/components/SModal.vue'
import SButton from '@qingfeng346/ui-kit/components/button/SButton.vue'
import { useAppVersion } from '@cliproam/ui/platform'
import { ArrowUpCircle, CheckCircle2, CircleAlert, Download } from 'lucide-vue-next'
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
const { appVersion, initAppVersion } = useAppVersion()
const { updateStatus, updateVersion, updateNotes, updateDownloaded, updateTotal, updateProgressPercent, updateMsg } = updater
const updateVisible = computed(() => ['available', 'downloading', 'installed', 'error'].includes(updateStatus.value))
const downloading = computed(() => updateStatus.value === 'downloading')
const title = computed(() => ({
  available: '发现新版本',
  downloading: '正在下载更新',
  installed: '更新已就绪',
  error: '更新失败',
}[updateStatus.value as 'available' | 'downloading' | 'installed' | 'error'] ?? '应用更新'))
const statusIcon = computed(() => ({
  available: ArrowUpCircle,
  downloading: Download,
  installed: CheckCircle2,
  error: CircleAlert,
}[updateStatus.value as 'available' | 'downloading' | 'installed' | 'error'] ?? ArrowUpCircle))
const updateErrorText = computed(() => {
  if (!updater.runningInTauri) return '浏览器预览环境不支持应用内更新。'
  if (!updater.updaterSupported) return '当前平台暂不支持应用内更新。'
  return updateMsg.value || '检查更新失败。'
})
const hasPendingUpdate = computed(() => Boolean(updater.pendingUpdate.value))
const renderedUpdateNotes = computed(() => DOMPurify.sanitize(marked.parse(updateNotes.value, { async: false }) as string))
const updateDate = computed(() => {
  const value = updater.pendingUpdate.value?.date
  if (!value) return ''
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: 'long', day: 'numeric' }).format(date)
})

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / 1024 / 1024).toFixed(1)} MB`
}

onMounted(() => {
  void initAppVersion()
  if (props.autoCheck) updater.scheduleAutoCheck(props.autoCheckDelayMs)
})
onBeforeUnmount(() => updater.dispose())

async function dismiss() {
  if (downloading.value) return
  updateStatus.value = 'idle'
  updateMsg.value = ''
  await updater.closePendingUpdate()
}
</script>

<template>
  <SModal
    :show="updateVisible"
    :title="title"
    width="640px"
    :closable="!downloading"
    :mask-closable="!downloading"
    :close-on-esc="!downloading"
    @update:show="(show: boolean) => { if (!show) dismiss() }"
  >
    <div class="updater-content">
      <div class="updater-summary" :class="{ 'is-error': updateStatus === 'error', 'is-installed': updateStatus === 'installed' }">
        <span class="updater-icon"><component :is="statusIcon" :size="24" aria-hidden="true" /></span>
        <div class="updater-summary-copy">
          <template v-if="updateStatus === 'available'">
            <strong class="updater-version">{{ updateVersion }}</strong>
            <p>{{ updateDate ? `发布于 ${updateDate}` : '新版本已可下载' }}</p>
          </template>
          <template v-else-if="downloading">
            <strong>{{ updateVersion ? `正在更新至 ${updateVersion}` : '正在下载更新包' }}</strong>
            <p>下载完成后将自动安装，请稍候。</p>
          </template>
          <template v-else-if="updateStatus === 'installed'">
            <strong>安装完成</strong>
            <p>重启应用即可使用新版本。</p>
          </template>
          <template v-else-if="updateStatus === 'error'">
            <strong>更新未能完成</strong>
            <p>请查看下方错误信息。</p>
          </template>
          <p>当前版本：{{ appVersion || '…' }}</p>
        </div>
      </div>

      <section v-if="updateStatus === 'available' && updateNotes" class="updater-release">
        <h3>更新内容</h3>
        <div class="updater-notes" v-html="renderedUpdateNotes" />
      </section>

      <div v-if="downloading" class="updater-download">
        <div class="updater-progress-label" aria-live="polite">
          <span>{{ updateTotal ? '下载进度' : '已下载' }}</span>
          <strong>{{ updateTotal ? `${updateProgressPercent}%` : formatBytes(updateDownloaded) }}</strong>
        </div>
        <div
          class="updater-progress"
          :class="{ 'is-indeterminate': !updateTotal }"
          role="progressbar"
          aria-label="更新下载进度"
          :aria-valuenow="updateTotal ? updateProgressPercent : undefined"
          :aria-valuemin="0"
          :aria-valuemax="100"
        >
          <div class="updater-progress-fill" :style="updateTotal ? { transform: `scaleX(${updateProgressPercent / 100})` } : undefined" />
        </div>
        <p v-if="updateTotal" class="updater-download-size">{{ formatBytes(updateDownloaded) }} / {{ formatBytes(updateTotal) }}</p>
      </div>

      <p v-if="updateStatus === 'error'" class="updater-error" role="alert">{{ updateErrorText }}</p>
    </div>

    <template #footer>
      <template v-if="updateStatus === 'available'">
        <SButton @click="dismiss">稍后</SButton>
        <SButton type="primary" @click="updater.downloadAndInstallUpdate">现在更新</SButton>
      </template>
      <SButton v-else-if="downloading" disabled>正在下载并安装…</SButton>
      <template v-else-if="updateStatus === 'installed'">
        <SButton @click="dismiss">稍后重启</SButton>
        <SButton type="primary" @click="updater.relaunchApp">立即重启</SButton>
      </template>
      <template v-else-if="updateStatus === 'error'">
        <SButton @click="dismiss">关闭</SButton>
        <SButton v-if="hasPendingUpdate" type="primary" @click="updater.downloadAndInstallUpdate">重试</SButton>
      </template>
    </template>
  </SModal>
</template>

<style scoped>
.updater-content { display: flex; flex-direction: column; gap: 20px; font-size: 13px; line-height: 1.6; }
.updater-summary { display: flex; align-items: center; gap: 14px; }
.updater-icon { display: flex; flex: 0 0 48px; height: 48px; align-items: center; justify-content: center; border: 1px solid var(--sui-border); border-radius: 12px; color: var(--sui-primary); background: var(--sui-bg-soft); }
.updater-summary.is-installed .updater-icon { color: var(--sui-success); }
.updater-summary.is-error .updater-icon { color: var(--sui-error); }
.updater-summary-copy { min-width: 0; }
.updater-summary-copy strong { display: block; font-size: 14px; font-weight: 600; overflow-wrap: anywhere; }
.updater-summary-copy .updater-version { font-size: 20px; line-height: 1.4; letter-spacing: -0.3px; }
.updater-summary-copy p { margin: 3px 0 0; color: var(--sui-fg-muted); }
.updater-release h3 { margin: 0 0 8px; font-size: 12px; font-weight: 600; color: var(--sui-fg-muted); }
.updater-notes { max-height: min(420px, 55vh); padding: 14px 16px; overflow: auto; overscroll-behavior: contain; overflow-wrap: anywhere; border: 1px solid var(--sui-border); border-radius: 8px; background: var(--sui-bg-subtle); }
.updater-notes :deep(*) { box-sizing: border-box; }
.updater-notes :deep(h1), .updater-notes :deep(h2), .updater-notes :deep(h3), .updater-notes :deep(h4), .updater-notes :deep(h5), .updater-notes :deep(h6) { margin: 16px 0 8px; font-size: 14px; font-weight: 600; color: var(--sui-fg); line-height: 1.4; }
.updater-notes :deep(> :first-child) { margin-top: 0; }
.updater-notes :deep(> :last-child) { margin-bottom: 0; }
.updater-notes :deep(p), .updater-notes :deep(ul), .updater-notes :deep(ol) { margin: 0 0 10px; }
.updater-notes :deep(ul), .updater-notes :deep(ol) { padding-left: 20px; }
.updater-notes :deep(li + li) { margin-top: 4px; }
.updater-notes :deep(code) { padding: 2px 5px; border-radius: 4px; background: var(--sui-bg-soft); font-family: ui-monospace, Consolas, monospace; font-size: 0.92em; }
.updater-notes :deep(pre) { max-width: 100%; padding: 10px 12px; overflow: auto; border-radius: 6px; background: var(--sui-bg-soft); }
.updater-notes :deep(pre code) { padding: 0; }
.updater-notes :deep(a) { color: var(--sui-primary); text-decoration: underline; text-underline-offset: 3px; }
.updater-notes :deep(img) { max-width: 100%; height: auto; }
.updater-notes :deep(blockquote) { margin: 10px 0; padding-left: 12px; border-left: 2px solid var(--sui-border-strong); color: var(--sui-fg-muted); }
.updater-progress-label { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 8px; color: var(--sui-fg-muted); }
.updater-progress-label strong { color: var(--sui-fg); font-variant-numeric: tabular-nums; }
.updater-progress { height: 6px; overflow: hidden; border-radius: 999px; background: var(--sui-bg-soft); }
.updater-progress-fill { height: 100%; border-radius: inherit; background: var(--sui-primary); transform-origin: left; transition: transform 200ms ease; }
.updater-progress.is-indeterminate .updater-progress-fill { width: 35%; animation: updater-progress 1.4s ease-in-out infinite; }
.updater-download-size { margin: 8px 0 0; font-size: 12px; color: var(--sui-fg-muted); text-align: right; font-variant-numeric: tabular-nums; }
.updater-error { margin: 0; padding: 12px 14px; border: 1px solid var(--sui-border); border-left: 3px solid var(--sui-error); border-radius: 6px; background: var(--sui-bg-subtle); color: var(--sui-error); overflow-wrap: anywhere; }
@keyframes updater-progress { from { transform: translateX(-100%); } to { transform: translateX(390%); } }
@media (prefers-reduced-motion: reduce) { .updater-progress-fill { transition: none; } .updater-progress.is-indeterminate .updater-progress-fill { animation: none; } }
</style>
