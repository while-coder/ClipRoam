<script setup lang="ts">
import { onMounted, ref } from "vue";
import { fetchStatus, updateTransferSettings } from "../../shared/api.js";

const submitting = ref(false);
const error = ref("");
const notice = ref("");
const maxStoredFileMb = ref(100);
const resumableUploadTtlHours = ref(24);

async function load(): Promise<void> {
  try {
    const result = await fetchStatus();
    maxStoredFileMb.value = result.transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = result.transfer.resumableUploadTtlHours;
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
    });
    maxStoredFileMb.value = transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = transfer.resumableUploadTtlHours;
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
            <label for="max-stored-file-mb">服务器文件上限（MB）</label>
            <input id="max-stored-file-mb" v-model.number="maxStoredFileMb" type="number" min="0" max="100000" step="1" :disabled="submitting" required />
            <p class="field-hint">设为 0 可禁止向服务器保存文件。</p>
          </div>
          <div>
            <label for="upload-resume-ttl">断点续传有效期（小时）</label>
            <input id="upload-resume-ttl" v-model.number="resumableUploadTtlHours" type="number" min="0" max="100000" step="1" :disabled="submitting" required />
            <p class="field-hint">设为 0 可禁用断点续传。</p>
          </div>
        </div>
        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <p v-if="notice" class="message success" role="status">{{ notice }}</p>
        <button type="submit" :disabled="submitting">{{ submitting ? "正在保存…" : "保存传输设置" }}</button>
      </form>
    </section>
  </section>
</template>
