<script setup lang="ts">
import { onMounted, ref } from "vue";
import { fetchStatus, removeTls, replaceTls, type TlsStatus } from "../../shared/api.js";

const submitting = ref(false);
const error = ref("");
const notice = ref("");
const status = ref<TlsStatus>();
const certificate = ref("");
const privateKey = ref("");
const confirmingTlsRemoval = ref(false);

async function load(): Promise<void> {
  try {
    status.value = (await fetchStatus()).tls;
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "加载证书状态失败。";
  }
}

async function save(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await replaceTls(certificate.value, privateKey.value);
    status.value = result.tls;
    certificate.value = "";
    privateKey.value = "";
    notice.value = result.restartRequired
      ? "证书已保存。当前服务仍是 HTTP，请重启服务后启用 HTTPS/WSS。"
      : "证书已更新，HTTPS/WSS 已使用新证书。";
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "保存证书失败。";
  } finally {
    submitting.value = false;
  }
}

function requestTlsRemoval(): void {
  error.value = "";
  notice.value = "";
  confirmingTlsRemoval.value = true;
}

async function remove(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await removeTls();
    status.value = result.tls;
    confirmingTlsRemoval.value = false;
    notice.value = "证书已删除。服务仍会维持当前 HTTPS 直到重启；重启后同一端口将回到 HTTP/WS。";
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "删除证书失败。";
  } finally {
    submitting.value = false;
  }
}

async function loadPem(event: Event, target: "certificate" | "privateKey"): Promise<void> {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  const text = await file.text();
  if (target === "certificate") certificate.value = text;
  else privateKey.value = text;
  input.value = "";
}

onMounted(load);
</script>

<template>
  <section class="content-section" aria-labelledby="security-title">
    <header class="topbar">
      <h1 id="security-title">HTTPS 证书</h1>
    </header>

    <section class="panel" aria-labelledby="tls-settings-title">
      <div class="panel-heading">
        <div>
          <h2 id="tls-settings-title">证书配置</h2>
          <p>上传 PEM 格式证书链与对应的私钥。私钥不会在后台再次显示。</p>
        </div>
      </div>

      <form @submit.prevent="save">
        <label for="certificate">证书或完整证书链</label>
        <input id="certificate-file" class="file-input" type="file" accept=".pem,.crt,.cer,text/plain" @change="loadPem($event, 'certificate')" />
        <textarea id="certificate" v-model="certificate" rows="7" spellcheck="false" placeholder="-----BEGIN CERTIFICATE-----" :disabled="submitting" required />

        <label for="private-key">私钥</label>
        <input id="private-key-file" class="file-input" type="file" accept=".pem,.key,text/plain" @change="loadPem($event, 'privateKey')" />
        <textarea id="private-key" v-model="privateKey" rows="7" spellcheck="false" placeholder="-----BEGIN PRIVATE KEY-----" :disabled="submitting" required />

        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <p v-if="notice" class="message success" role="status">{{ notice }}</p>
        <div class="form-actions">
          <button type="submit" :disabled="submitting">
            {{ submitting ? "正在校验证书…" : status?.source === "managed" ? "替换证书" : "保存证书" }}
          </button>
          <button v-if="status?.source === 'managed'" class="danger" type="button" :disabled="submitting" @click="requestTlsRemoval">删除证书</button>
        </div>
      </form>
    </section>

    <div v-if="confirmingTlsRemoval" class="modal-backdrop" role="presentation">
      <section class="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-tls-title">
        <h2 id="delete-tls-title">删除 HTTPS 证书？</h2>
        <p>证书和私钥会从服务端数据目录删除。重启服务后，同一端口将回到 HTTP/WS。</p>
        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <div class="form-actions">
          <button class="secondary" type="button" :disabled="submitting" @click="confirmingTlsRemoval = false">取消</button>
          <button class="danger" type="button" :disabled="submitting" @click="remove">{{ submitting ? "正在删除…" : "确认删除" }}</button>
        </div>
      </section>
    </div>

    <p class="warning">首次从 HTTP 配置证书时，提交请求本身仍未加密。请只在受信任网络中操作，并在保存后立即重启服务。</p>
  </section>
</template>
