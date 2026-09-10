<script setup lang="ts">
import { onMounted, ref } from "vue";
import { fetchStatus, updateTransferSettings } from "../../shared/api.js";

// The dropdown presets live here; the server only sanity-checks that a saved
// value stays inside its range (ServerConfig.ts).
const MAX_STORED_FILE_MB_OPTIONS = [0, 100, 200, 300, 400, 500, 1024, 1536, 2048];
const RESUMABLE_UPLOAD_TTL_HOURS_OPTIONS = [0, 1, 6, 12, 24, 48, 72, 168];
const MAX_HISTORY_ENTRIES_OPTIONS = [1000, 1500, 2000, 2500, 3000];
const MAX_CAPTURE_FILE_COUNT_OPTIONS = [100, 300, 500, 1000, 2000, 5000];

const submitting = ref(false);
const error = ref("");
const notice = ref("");
const maxStoredFileMb = ref(100);
const resumableUploadTtlHours = ref(24);
const maxHistoryEntries = ref(3000);
const maxCaptureFileCount = ref(1000);

function maxStoredFileMbLabel(value: number): string {
  if (value === 0) return "0（禁止保存文件）";
  if (value === 2048) return "2048 MB（2 GB）";
  return `${value} MB`;
}

function resumableUploadTtlHoursLabel(value: number): string {
  if (value === 0) return "0（禁用断点续传）";
  if (value === 168) return "168 小时（7 天）";
  return `${value} 小时`;
}

async function load(): Promise<void> {
  try {
    const result = await fetchStatus();
    maxStoredFileMb.value = result.transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = result.transfer.resumableUploadTtlHours;
    maxHistoryEntries.value = result.transfer.maxHistoryEntries;
    maxCaptureFileCount.value = result.transfer.maxCaptureFileCount;
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "加载传输设置失败。";
  }
}

async function save(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    const transfer = await updateTransferSettings({
      maxStoredFileMb: maxStoredFileMb.value,
      resumableUploadTtlHours: resumableUploadTtlHours.value,
      maxHistoryEntries: maxHistoryEntries.value,
      maxCaptureFileCount: maxCaptureFileCount.value,
    });
    maxStoredFileMb.value = transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = transfer.resumableUploadTtlHours;
    maxHistoryEntries.value = transfer.maxHistoryEntries;
    maxCaptureFileCount.value = transfer.maxCaptureFileCount;
    notice.value = "传输设置已保存，并已应用到后续上传与续传。";
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "保存传输设置失败。";
  } finally {
    submitting.value = false;
  }
}

onMounted(load);
</script>

<template>
  <section class="content-section" aria-labelledby="transfer-title">
    <header class="topbar">
      <h1 id="transfer-title">文件传输</h1>
    </header>

    <section class="panel" aria-labelledby="transfer-settings-title">
      <div class="panel-heading">
        <div>
          <h2 id="transfer-settings-title">传输设置</h2>
          <p>修改后立即影响新上传与下一次断点续传，不会中断正在传输的文件。</p>
        </div>
      </div>

      <form @submit.prevent="save">
        <div class="form-grid">
          <div>
            <label for="max-stored-file-mb">服务器文件上限</label>
            <select id="max-stored-file-mb" v-model.number="maxStoredFileMb" :disabled="submitting" required>
              <option v-for="value in MAX_STORED_FILE_MB_OPTIONS" :key="value" :value="value">{{ maxStoredFileMbLabel(value) }}</option>
            </select>
            <p class="field-hint">限制单文件可保存到服务器的最大体积，设为 0 可禁止向服务器保存文件。</p>
          </div>
          <div>
            <label for="upload-resume-ttl">断点续传有效期</label>
            <select id="upload-resume-ttl" v-model.number="resumableUploadTtlHours" :disabled="submitting" required>
              <option v-for="value in RESUMABLE_UPLOAD_TTL_HOURS_OPTIONS" :key="value" :value="value">{{ resumableUploadTtlHoursLabel(value) }}</option>
            </select>
            <p class="field-hint">设为 0 可禁用断点续传。</p>
          </div>
          <div>
            <label for="max-history-entries">单用户最大历史条数</label>
            <select id="max-history-entries" v-model.number="maxHistoryEntries" :disabled="submitting" required>
              <option v-for="value in MAX_HISTORY_ENTRIES_OPTIONS" :key="value" :value="value">{{ value }} 条</option>
            </select>
            <p class="field-hint">超过上限后服务器自动删除最旧的剪贴板记录，并广播 clipboard.deleted 让各设备同步删除本地副本。</p>
          </div>
          <div>
            <label for="max-capture-file-count">单次复制文件数上限</label>
            <select id="max-capture-file-count" v-model.number="maxCaptureFileCount" :disabled="submitting" required>
              <option v-for="value in MAX_CAPTURE_FILE_COUNT_OPTIONS" :key="value" :value="value">{{ value }} 个</option>
            </select>
            <p class="field-hint">单次复制的文件夹超过该文件数量时，设备不捕获、不同步该内容。</p>
          </div>
        </div>
        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <p v-if="notice" class="message success" role="status">{{ notice }}</p>
        <button type="submit" :disabled="submitting">{{ submitting ? "正在保存…" : "保存传输设置" }}</button>
      </form>
    </section>
  </section>
</template>
