import { computed, ref, shallowRef, type ComputedRef, type Ref, type ShallowRef } from 'vue'
import { isTauri } from '@tauri-apps/api/core'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater'

export type TauriUpdaterStatus =
  | 'idle'
  | 'checking'
  | 'latest'
  | 'available'
  | 'downloading'
  | 'installed'
  | 'error'

export interface CheckForUpdateOptions {
  silent?: boolean
}

export type CheckForUpdateResult = 'available' | 'latest' | 'unsupported' | 'error'

export interface CreateTauriUpdaterOptions {
  autoCheckDelayMs?: number
  isMobileRuntime?: () => boolean
  logger?: Pick<Console, 'warn'>
}

export interface TauriUpdater {
  runningInTauri: boolean
  updaterSupported: boolean
  updateStatus: Ref<TauriUpdaterStatus>
  updateStatusText: ComputedRef<string>
  updateMsg: Ref<string>
  pendingUpdate: ShallowRef<Update | null>
  updateVersion: Ref<string>
  updateNotes: Ref<string>
  updateDownloaded: Ref<number>
  updateTotal: Ref<number | null>
  updateProgressPercent: ComputedRef<number>
  showAutoUpdatePrompt: ComputedRef<boolean>
  checkForUpdate: (options?: CheckForUpdateOptions) => Promise<CheckForUpdateResult>
  scheduleAutoCheck: (delayMs?: number) => void
  stopAutoCheck: () => void
  downloadAndInstallUpdate: () => Promise<void>
  relaunchApp: () => Promise<void>
  closePendingUpdate: () => Promise<void>
  dismissAutoUpdatePrompt: () => void
  showLatestStatus: () => void
  dispose: () => void
}

function defaultIsMobileRuntime() {
  return /android|iphone|ipad|ipod/i.test(window.navigator.userAgent)
}

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / 1024 / 1024).toFixed(1)} MB`
}

function getErrorMessage(e: unknown) {
  return e instanceof Error ? e.message : String(e)
}

export function createTauriUpdater(options: CreateTauriUpdaterOptions = {}): TauriUpdater {
  const logger = options.logger ?? console
  const runningInTauri = isTauri()
  const isMobileRuntime = options.isMobileRuntime ?? defaultIsMobileRuntime
  const updaterSupported = runningInTauri && !isMobileRuntime()
  const updateStatus = ref<TauriUpdaterStatus>('idle')
  const updateMsg = ref('')
  const pendingUpdate = shallowRef<Update | null>(null)
  const updateVersion = ref('')
  const updateNotes = ref('')
  const updateDownloaded = ref(0)
  const updateTotal = ref<number | null>(null)
  const autoCheckDone = ref(false)
  const autoCheckDismissed = ref(false)
  let checkInFlight: Promise<CheckForUpdateResult> | null = null
  let autoCheckTimer: ReturnType<typeof window.setTimeout> | null = null

  const updateProgressPercent = computed(() => {
    if (!updateTotal.value) return 0
    return Math.min(100, Math.round((updateDownloaded.value / updateTotal.value) * 100))
  })

  const updateStatusText = computed(() => {
    if (!runningInTauri) return '浏览器预览环境不支持应用内更新。'
    if (!updaterSupported) return '当前平台暂不支持应用内更新。'
    if (updateStatus.value === 'checking') return '正在检查更新...'
    if (updateStatus.value === 'latest') return '当前已是最新版本。'
    if (updateStatus.value === 'available') return `发现新版本 ${updateVersion.value}。`
    if (updateStatus.value === 'downloading') {
      if (updateTotal.value) return `正在下载更新：${updateProgressPercent.value}%`
      return `正在下载更新：${formatBytes(updateDownloaded.value)}`
    }
    if (updateStatus.value === 'installed') return '更新已安装，重启后生效。'
    if (updateStatus.value === 'error') return updateMsg.value || '检查更新失败。'
    return '手动检查桌面端安装包更新。'
  })

  const showAutoUpdatePrompt = computed(
    () => updateStatus.value === 'available' && !autoCheckDismissed.value,
  )

  function resetDownloadState() {
    updateDownloaded.value = 0
    updateTotal.value = null
  }

  function onDownloadEvent(event: DownloadEvent) {
    if (event.event === 'Started') {
      updateTotal.value = event.data.contentLength ?? null
      updateDownloaded.value = 0
    } else if (event.event === 'Progress') {
      updateDownloaded.value += event.data.chunkLength
    }
  }

  async function closePendingUpdate() {
    const update = pendingUpdate.value
    pendingUpdate.value = null
    if (update) await update.close().catch(() => undefined)
  }

  async function checkForUpdate(checkOptions: CheckForUpdateOptions = {}) {
    if (checkInFlight) return checkInFlight

    checkInFlight = (async () => {
      updateMsg.value = ''
      updateVersion.value = ''
      updateNotes.value = ''
      resetDownloadState()
      await closePendingUpdate()

      if (!updaterSupported) {
        if (!checkOptions.silent) {
          updateStatus.value = 'error'
          updateMsg.value = ''
        }
        return 'unsupported'
      }

      updateStatus.value = 'checking'
      try {
        const update = await check()
        if (!update) {
          updateStatus.value = 'latest'
          return 'latest'
        }
        pendingUpdate.value = update
        updateVersion.value = update.version
        updateNotes.value = update.body?.trim() || ''
        updateStatus.value = 'available'
        autoCheckDismissed.value = false
        return 'available'
      } catch (e) {
        if (checkOptions.silent) {
          updateStatus.value = 'idle'
          logger.warn('[updater] check failed:', e)
        } else {
          updateStatus.value = 'error'
          updateMsg.value = getErrorMessage(e)
        }
        return 'error'
      }
    })().finally(() => {
      checkInFlight = null
    })

    return checkInFlight
  }

  function stopAutoCheck() {
    if (autoCheckTimer === null) return
    window.clearTimeout(autoCheckTimer)
    autoCheckTimer = null
  }

  function scheduleAutoCheck(delayMs = options.autoCheckDelayMs ?? 5000) {
    if (autoCheckDone.value || !updaterSupported || autoCheckTimer !== null) return
    autoCheckTimer = window.setTimeout(() => {
      autoCheckTimer = null
      autoCheckDone.value = true
      void checkForUpdate({ silent: true })
    }, delayMs)
  }

  async function downloadAndInstallUpdate() {
    const update = pendingUpdate.value
    if (!update) return
    updateStatus.value = 'downloading'
    updateMsg.value = ''
    resetDownloadState()
    try {
      await update.downloadAndInstall(onDownloadEvent)
      await closePendingUpdate()
      updateStatus.value = 'installed'
      autoCheckDismissed.value = true
    } catch (e) {
      updateStatus.value = 'error'
      updateMsg.value = getErrorMessage(e)
    }
  }

  async function relaunchApp() {
    updateMsg.value = ''
    try {
      await relaunch()
    } catch (e) {
      updateStatus.value = 'error'
      updateMsg.value = getErrorMessage(e)
    }
  }

  function dismissAutoUpdatePrompt() {
    autoCheckDismissed.value = true
  }

  function showLatestStatus() {
    updateMsg.value = ''
    updateVersion.value = ''
    updateNotes.value = ''
    resetDownloadState()
    updateStatus.value = 'latest'
  }

  function dispose() {
    stopAutoCheck()
    void closePendingUpdate()
  }

  return {
    runningInTauri,
    updaterSupported,
    updateStatus,
    updateStatusText,
    updateMsg,
    pendingUpdate,
    updateVersion,
    updateNotes,
    updateDownloaded,
    updateTotal,
    updateProgressPercent,
    showAutoUpdatePrompt,
    checkForUpdate,
    scheduleAutoCheck,
    stopAutoCheck,
    downloadAndInstallUpdate,
    relaunchApp,
    closePendingUpdate,
    dismissAutoUpdatePrompt,
    showLatestStatus,
    dispose,
  }
}

let defaultUpdater: TauriUpdater | null = null

export function useTauriUpdater(options: CreateTauriUpdaterOptions = {}) {
  if (!defaultUpdater) defaultUpdater = createTauriUpdater(options)
  return defaultUpdater
}
