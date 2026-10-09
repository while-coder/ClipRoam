<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { toast } from "@qingfeng346/ui-kit";
import { fetchStatus, updateTransferSettings } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

// The dropdown presets live here; the server only sanity-checks that a saved
// value stays inside its range (ServerConfig.ts).
const MAX_STORED_FILE_MB_OPTIONS = [0, 100, 200, 300, 400, 500, 1024, 1536, 2048];
const RESUMABLE_UPLOAD_TTL_HOURS_OPTIONS = [0, 1, 6, 12, 24, 48, 72, 168];
const MAX_HISTORY_ENTRIES_OPTIONS = [1000, 1500, 2000, 2500, 3000];
const MAX_CAPTURE_FILE_COUNT_OPTIONS = [100, 300, 500, 1000, 2000, 5000];

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

// SSelect 的 options 数组（label 展示 / value 取值），由预设数组派生。
const maxStoredFileMbSelect = computed(() =>
  MAX_STORED_FILE_MB_OPTIONS.map((value) => ({ label: maxStoredFileMbLabel(value), value })),
);
const resumableUploadTtlHoursSelect = computed(() =>
  RESUMABLE_UPLOAD_TTL_HOURS_OPTIONS.map((value) => ({ label: resumableUploadTtlHoursLabel(value), value })),
);
const maxHistoryEntriesSelect = computed(() =>
  MAX_HISTORY_ENTRIES_OPTIONS.map((value) => ({ label: `${value} 条`, value })),
);
const maxCaptureFileCountSelect = computed(() =>
  MAX_CAPTURE_FILE_COUNT_OPTIONS.map((value) => ({ label: `${value} 个`, value })),
);

const submitting = ref(false);
const error = ref("");
const maxStoredFileMb = ref(100);
const resumableUploadTtlHours = ref(24);
const maxHistoryEntries = ref(3000);
const maxCaptureFileCount = ref(1000);

async function load(): Promise<void> {
  try {
    const result = await fetchStatus();
    maxStoredFileMb.value = result.transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = result.transfer.resumableUploadTtlHours;
    maxHistoryEntries.value = result.transfer.maxHistoryEntries;
    maxCaptureFileCount.value = result.transfer.maxCaptureFileCount;
  } catch (reason) {
    error.value = errorMessage(reason, "加载传输设置失败。");
  }
}

async function save(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
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
    toast.show("success", "传输设置已保存，并已应用到后续上传与续传。");
  } catch (reason) {
    error.value = errorMessage(reason, "保存传输设置失败。");
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

    <SCard title="传输设置">
      <p class="muted panel-desc">修改后立即影响新上传与下一次断点续传，不会中断正在传输的文件。</p>

      <SForm>
        <div class="form-grid">
          <SFormItem label="服务器文件上限" hint="限制单文件可保存到服务器的最大体积，设为 0 可禁止向服务器保存文件。">
            <SSelect id="max-stored-file-mb" v-model:value="maxStoredFileMb" :options="maxStoredFileMbSelect" :disabled="submitting" required />
          </SFormItem>
          <SFormItem label="断点续传有效期" hint="设为 0 可禁用断点续传。">
            <SSelect id="upload-resume-ttl" v-model:value="resumableUploadTtlHours" :options="resumableUploadTtlHoursSelect" :disabled="submitting" required />
          </SFormItem>
          <SFormItem
            label="单用户最大历史条数"
            hint="超过上限后服务器自动删除最旧的剪贴板记录，并广播 clipboard.deleted 让各设备同步删除本地副本。"
          >
            <SSelect id="max-history-entries" v-model:value="maxHistoryEntries" :options="maxHistoryEntriesSelect" :disabled="submitting" required />
          </SFormItem>
          <SFormItem label="单次复制文件数上限" hint="单次复制的文件夹超过该文件数量时，设备不捕获、不同步该内容。">
            <SSelect id="max-capture-file-count" v-model:value="maxCaptureFileCount" :options="maxCaptureFileCountSelect" :disabled="submitting" required />
          </SFormItem>
        </div>
        <SAlert v-if="error" type="error">{{ error }}</SAlert>
        <SButton type="primary" :loading="submitting" @click="save">保存传输设置</SButton>
      </SForm>
    </SCard>
  </section>
</template>