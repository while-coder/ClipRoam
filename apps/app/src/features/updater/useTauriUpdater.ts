import { computed, ref, shallowRef, type ComputedRef, type Ref, type ShallowRef } from 'vue'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater'

/** 与 Rust 侧 platforms/mobile/android/updater.rs 的 UPDATE_PROGRESS_EVENT 保持一致。 */
const UPDATE_PROGRESS_EVENT = 'cliproam://update-progress'

export type TauriUpdaterStatus =
  | 'idle'
  | 'checking'
  | 'latest'
  | 'available'
  | 'downloading'
  | 'installed'
  | 'permission'
  | 'error'

export interface CheckForUpdateOptions {
  silent?: boolean
}

export type CheckForUpdateResult = 'available' | 'latest' | 'unsupported' | 'permission' | 'error'

export interface AndroidUpdateInfo {
  version: string
  notes: string
  url: string
}

export interface CreateTauriUpdaterOptions {
  autoCheckDelayMs?: number
  isMobileRuntime?: () => boolean
  isAndroidRuntime?: () => boolean
  logger?: Pick<Console, 'warn'>
}

export interface TauriUpdater {
  runningInTauri: boolean
  updaterSupported: boolean
  /** Android 走自建更新流程（下载 APK 后唤起系统安装器），桌面端走 tauri-plugin-updater。 */
  androidUpdater: boolean
  updateStatus: Ref<TauriUpdaterStatus>
  updateStatusText: ComputedRef<string>
  updateMsg: Ref<string>
  pendingUpdate: ShallowRef<Update | null>
  /** Android 更新包下载地址，非空表示存在待下载/安装的 Android 更新。 */
  androidUpdateUrl: Ref<string>
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
  requestInstallPermission: () => Promise<void>
  relaunchApp: () => Promise<void>
  closePendingUpdate: () => Promise<void>
  dismissAutoUpdatePrompt: () => void
  showLatestStatus: () => void
  dispose: () => void
}

function defaultIsMobileRuntime() {
  return /android|iphone|ipad|ipod/i.test(window.navigator.userAgent)
}

function defaultIsAndroidRuntime() {
  return /android/i.test(window.navigator.userAgent)
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
  // 桌面端走 tauri-plugin-updater；Android 走自建的「权限检测 -> 下载 APK -> 系统安装器」
  // 流程（iOS 无法侧载更新，保持不支持）。
  const isAndroidRuntime = options.isAndroidRuntime ?? defaultIsAndroidRuntime
  const isAndroid = runningInTauri && isAndroidRuntime()
  const updaterSupported = runningInTauri && (!isMobileRuntime() || isAndroid)
  const updateStatus = ref<TauriUpdaterStatus>('idle')
  const updateMsg = ref('')
  const pendingUpdate = shallowRef<Update | null>(null)
  const updateVersion = ref('')
  const updateNotes = ref('')
  const updateDownloaded = ref(0)
  const updateTotal = ref<number | null>(null)
  const autoCheckDone = ref(false)
  const autoCheckDismissed = ref(false)
  const androidUpdateUrl = ref('')
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
    if (updateStatus.value === 'installed') {
      return isAndroid ? '更新包已下载，请在系统安装提示中完成安装。' : '更新已安装，重启后生效。'
    }
    if (updateStatus.value === 'permission') {
      return '需要开启「安装未知应用」权限才能检查更新。'
    }
    if (updateStatus.value === 'error') return updateMsg.value || '检查更新失败。'
    return isAndroid ? '手动检查 Android 安装包更新。' : '手动检查桌面端安装包更新。'
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
      androidUpdateUrl.value = ''
      resetDownloadState()
      await closePendingUpdate()

      if (!updaterSupported) {
        if (!checkOptions.silent) {
          updateStatus.value = 'error'
          updateMsg.value = ''
        }
        return 'unsupported'
      }

      // Android：先检测「安装未知应用」权限，无权限时引导授权，有权限才自检。
      if (isAndroid) {
        updateStatus.value = 'checking'
        try {
          if (!(await invoke<boolean>('native_update_permission_status'))) {
            updateStatus.value = 'permission'
            return 'permission'
          }
          const info = await invoke<AndroidUpdateInfo | null>('native_update_check')
          if (!info) {
            updateStatus.value = 'latest'
            return 'latest'
          }
          androidUpdateUrl.value = info.url
          updateVersion.value = info.version
          updateNotes.value = info.notes
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
    if (isAndroid) return downloadAndInstallAndroidUpdate()
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

  /** Android：由 Rust 下载 APK（事件上报进度），下载完成后唤起系统安装器。 */
  async function downloadAndInstallAndroidUpdate() {
    const url = androidUpdateUrl.value
    if (!url) return
    updateStatus.value = 'downloading'
    updateMsg.value = ''
    resetDownloadState()
    let unlisten: (() => void) | null = null
    try {
      unlisten = await listen<{ downloaded: number; total: number | null }>(UPDATE_PROGRESS_EVENT, (event) => {
        updateDownloaded.value = event.payload.downloaded
        updateTotal.value = event.payload.total ?? null
      })
      const path = await invoke<string>('native_update_download', {
        url,
        version: updateVersion.value,
      })
      await invoke('native_update_install', { path })
      updateStatus.value = 'installed'
      autoCheckDismissed.value = true
    } catch (e) {
      updateStatus.value = 'error'
      updateMsg.value = getErrorMessage(e)
    } finally {
      unlisten?.()
    }
  }

  async function requestInstallPermission() {
    updateMsg.value = ''
    try {
      await invoke('native_update_request_permission')
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
    androidUpdater: isAndroid,
    updateStatus,
    updateStatusText,
    updateMsg,
    pendingUpdate,
    androidUpdateUrl,
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
    requestInstallPermission,
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
