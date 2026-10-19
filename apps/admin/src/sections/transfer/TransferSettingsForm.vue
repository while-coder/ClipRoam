<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { toast } from "@qingfeng346/ui-kit";
import { fetchStatus, updateTransferSettings, fetchUserTransferSettings, updateUserTransferSettings, resetUserTransferSettings, type TransferSettings, type TransferOverrides, type UserTransferSettings } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

const props = defineProps<{ userId?: string }>();
const emit = defineEmits<{ busy: [value: boolean] }>();

// The dropdown presets live here; the server only sanity-checks that a saved
// value stays inside its range (ServerConfig.ts).
const MAX_STORED_FILE_MB_OPTIONS = [0, 50, 100, 200, 300, 400, 500, 1024, 1536, 2048];
const MAX_STORED_IMAGE_MB_OPTIONS = [0, 5, 10, 20, 50, 100, 200];
const RESUMABLE_UPLOAD_TTL_HOURS_OPTIONS = [0, 1, 6, 12, 24, 48, 72, 168];
const MAX_HISTORY_ENTRIES_OPTIONS = [100, 200, 500, 1000, 1500, 2000, 2500, 3000];
const MAX_CAPTURE_FILE_COUNT_OPTIONS = [100, 300, 500, 1000, 2000, 5000];

function maxStoredFileMbLabel(value: number): string {
  if (value === 0) return "0（禁止保存普通文件）";
  if (value === 2048) return "2048 MB（2 GB）";
  return `${value} MB`;
}

function resumableUploadTtlHoursLabel(value: number): string {
  if (value === 0) return "0（禁用断点续传）";
  if (value === 168) return "168 小时（7 天）";
  return `${value} 小时`;
}

type SettingValue = number | "global";
const globalDefaults = ref<TransferSettings>();

function options(presets: number[], current: SettingValue, key: keyof TransferSettings, label: (value: number) => string) {
  const values = [...new Set([...presets, ...(typeof current === "number" ? [current] : [])])].sort((a, b) => a - b);
  const choices: Array<{ label: string; value: SettingValue }> = values.map((value) => ({ label: label(value), value }));
  if (props.userId) {
    const defaultValue = globalDefaults.value?.[key];
    choices.unshift({ label: defaultValue === undefined ? "全局配置" : `全局配置（当前 ${label(defaultValue)}）`, value: "global" });
  }
  return choices;
}

const maxStoredFileMbSelect = computed(() => options(MAX_STORED_FILE_MB_OPTIONS, maxStoredFileMb.value, "maxStoredFileMb", maxStoredFileMbLabel));
const maxStoredImageMbSelect = computed(() => options(MAX_STORED_IMAGE_MB_OPTIONS, maxStoredImageMb.value, "maxStoredImageMb", (value) => value === 0 ? "0（禁止保存剪贴板图片）" : `${value} MB`));
const resumableUploadTtlHoursSelect = computed(() => options(RESUMABLE_UPLOAD_TTL_HOURS_OPTIONS, resumableUploadTtlHours.value, "resumableUploadTtlHours", resumableUploadTtlHoursLabel));
const maxHistoryEntriesSelect = computed(() => options(MAX_HISTORY_ENTRIES_OPTIONS, maxHistoryEntries.value, "maxHistoryEntries", (value) => `${value} 条`));
const maxCaptureFileCountSelect = computed(() => options(MAX_CAPTURE_FILE_COUNT_OPTIONS, maxCaptureFileCount.value, "maxCaptureFileCount", (value) => `${value} 个`));

const submitting = ref(false);
const loading = ref(true);
const loaded = ref(false);
const customized = ref(false);
const disabled = computed(() => loading.value || !loaded.value || submitting.value);
const error = ref("");
const maxStoredFileMb = ref<SettingValue>(100);
const maxStoredImageMb = ref<SettingValue>(10);
const resumableUploadTtlHours = ref<SettingValue>(24);
const maxHistoryEntries = ref<SettingValue>(3000);
const maxCaptureFileCount = ref<SettingValue>(1000);

function apply(transfer: TransferSettings, overrides?: TransferOverrides): void {
  maxStoredFileMb.value = props.userId ? overrides?.maxStoredFileMb ?? "global" : transfer.maxStoredFileMb;
  maxStoredImageMb.value = props.userId ? overrides?.maxStoredImageMb ?? "global" : transfer.maxStoredImageMb;
  resumableUploadTtlHours.value = props.userId ? overrides?.resumableUploadTtlHours ?? "global" : transfer.resumableUploadTtlHours;
  maxHistoryEntries.value = props.userId ? overrides?.maxHistoryEntries ?? "global" : transfer.maxHistoryEntries;
  maxCaptureFileCount.value = props.userId ? overrides?.maxCaptureFileCount ?? "global" : transfer.maxCaptureFileCount;
}

function applyUser(result: UserTransferSettings): void {
  globalDefaults.value = result.defaults;
  apply(result.transfer, result.overrides);
  customized.value = result.customized;
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    if (props.userId) {
      applyUser(await fetchUserTransferSettings(props.userId));
    } else {
      apply((await fetchStatus()).transfer);
    }
    loaded.value = true;
  } catch (reason) {
    error.value = errorMessage(reason, "加载传输设置失败。");
  } finally {
    loading.value = false;
  }
}

async function save(): Promise<void> {
  if (disabled.value) return;
  submitting.value = true;
  emit("busy", true);
  error.value = "";
  try {
    const settings = {
      maxStoredFileMb: maxStoredFileMb.value,
      maxStoredImageMb: maxStoredImageMb.value,
      resumableUploadTtlHours: resumableUploadTtlHours.value,
      maxHistoryEntries: maxHistoryEntries.value,
      maxCaptureFileCount: maxCaptureFileCount.value,
    };
    const overrides = Object.fromEntries(Object.entries(settings).filter(([, value]) => typeof value === "number")) as TransferOverrides;
    if (props.userId) {
      applyUser(await updateUserTransferSettings(props.userId, overrides));
    } else {
      apply(await updateTransferSettings(overrides as TransferSettings));
    }
    toast.show("success", props.userId ? "该账号的传输设置已保存。" : "全局传输设置已保存。");
  } catch (reason) {
    error.value = errorMessage(reason, "保存传输设置失败。");
  } finally {
    submitting.value = false;
    emit("busy", false);
  }
}

async function reset(): Promise<void> {
  if (!props.userId || disabled.value) return;
  submitting.value = true;
  emit("busy", true);
  error.value = "";
  try {
    applyUser(await resetUserTransferSettings(props.userId));
    toast.show("success", "该账号已恢复使用全局默认设置。");
  } catch (reason) {
    error.value = errorMessage(reason, "恢复全局默认失败。");
  } finally {
    submitting.value = false;
    emit("busy", false);
  }
}

onMounted(load);
</script>

<template>
  <div class="transfer-settings-form">
    <p v-if="userId && loaded" class="muted panel-desc">
      每项选择“全局配置”可跟随全局设置更新；选择具体数值则仅对此账号生效。
    </p>
    <p class="muted panel-desc">修改后立即影响新上传与下一次断点续传，不会中断正在传输的文件。</p>
    <p v-if="loading" class="muted" role="status">正在加载传输设置…</p>
    <SForm>
        <div class="form-grid">
          <SFormItem label="普通文件存储上限" hint="普通文件和文件夹中的单个文件须小于此上限。设为 0 禁止保存普通文件，不影响剪贴板图片。">
            <SSelect id="max-stored-file-mb" v-model:value="maxStoredFileMb" :options="maxStoredFileMbSelect" :disabled="disabled" required />
          </SFormItem>
          <SFormItem label="剪贴板图片存储上限" hint="截图等剪贴板图片小于此上限时自动上传，不受设备的普通文件自动上传设置影响。设为 0 禁止保存图片；在线中转仍可用。">
            <SSelect id="max-stored-image-mb" v-model:value="maxStoredImageMb" :options="maxStoredImageMbSelect" :disabled="disabled" required />
          </SFormItem>
          <SFormItem label="断点续传有效期" hint="设为 0 可禁用断点续传。">
            <SSelect id="upload-resume-ttl" v-model:value="resumableUploadTtlHours" :options="resumableUploadTtlHoursSelect" :disabled="disabled" required />
          </SFormItem>
          <SFormItem
            label="单用户最大历史条数"
            hint="仅计算未收藏记录，超过上限后自动删除最旧记录并同步到各设备；已收藏记录不自动清理。"
          >
            <SSelect id="max-history-entries" v-model:value="maxHistoryEntries" :options="maxHistoryEntriesSelect" :disabled="disabled" required />
          </SFormItem>
          <SFormItem label="单次复制文件数上限" hint="单次复制的文件夹超过该文件数量时，设备不捕获、不同步该内容。">
            <SSelect id="max-capture-file-count" v-model:value="maxCaptureFileCount" :options="maxCaptureFileCountSelect" :disabled="disabled" required />
          </SFormItem>
        </div>
        <SAlert v-if="error" type="error">{{ error }}</SAlert>
        <SButton type="primary" :disabled="disabled" :loading="submitting" @click="save">{{ userId ? '保存账号传输设置' : '保存传输设置' }}</SButton>
        <SButton v-if="userId" :disabled="disabled || !customized" @click="reset">恢复全局默认</SButton>
        <SButton v-if="!loading && !loaded" @click="load">重新加载</SButton>
    </SForm>
  </div>
</template>

<style scoped>
.transfer-settings-form { display: grid; gap: 12px; }
</style>
