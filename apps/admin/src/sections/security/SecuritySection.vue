<script setup lang="ts">
import { onMounted, ref } from "vue";
import { confirm, toast } from "@qingfeng346/ui-kit";
import { fetchStatus, removeTls, replaceTls, type TlsStatus } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

const submitting = ref(false);
const error = ref("");
const status = ref<TlsStatus>();
const certificate = ref("");
const privateKey = ref("");

async function load(): Promise<void> {
  try {
    status.value = (await fetchStatus()).tls;
  } catch (reason) {
    error.value = errorMessage(reason, "加载证书状态失败。");
  }
}

async function save(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  try {
    const result = await replaceTls(certificate.value, privateKey.value);
    status.value = result.tls;
    certificate.value = "";
    privateKey.value = "";
    toast.show(
      "success",
      result.restartRequired
        ? "证书已保存。当前服务仍是 HTTP，请重启服务后启用 HTTPS/WSS。"
        : "证书已更新，HTTPS/WSS 已使用新证书。",
    );
  } catch (reason) {
    error.value = errorMessage(reason, "保存证书失败。");
  } finally {
    submitting.value = false;
  }
}

// 删除确认走 kit confirm；失败时错误仍显示在下方 SAlert。
async function requestTlsRemoval(): Promise<void> {
  if (submitting.value) return;
  error.value = "";
  const confirmed = await confirm.show({
    title: "删除 HTTPS 证书？",
    content: "证书和私钥会从服务端数据目录删除。重启服务后，同一端口将回到 HTTP/WS。",
    confirmText: "确认删除",
    error: true,
  });
  if (!confirmed) return;
  submitting.value = true;
  error.value = "";
  try {
    const result = await removeTls();
    status.value = result.tls;
    toast.show("success", "证书已删除。服务仍会维持当前 HTTPS 直到重启；重启后同一端口将回到 HTTP/WS。");
  } catch (reason) {
    error.value = errorMessage(reason, "删除证书失败。");
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

    <SCard title="证书配置">
      <p class="muted panel-desc">上传 PEM 格式证书链与对应的私钥。私钥不会在后台再次显示。</p>

      <SForm>
        <SFormItem label="证书或完整证书链">
          <input id="certificate-file" class="file-input" type="file" accept=".pem,.crt,.cer,text/plain" @change="loadPem($event, 'certificate')" />
          <STextarea id="certificate" v-model:value="certificate" :rows="7" spellcheck="false" placeholder="-----BEGIN CERTIFICATE-----" :disabled="submitting" required />
        </SFormItem>
        <SFormItem label="私钥">
          <input id="private-key-file" class="file-input" type="file" accept=".pem,.key,text/plain" @change="loadPem($event, 'privateKey')" />
          <STextarea id="private-key" v-model:value="privateKey" :rows="7" spellcheck="false" placeholder="-----BEGIN PRIVATE KEY-----" :disabled="submitting" required />
        </SFormItem>

        <SAlert v-if="error" type="error">{{ error }}</SAlert>
        <SSpace>
          <SButton type="primary" :loading="submitting" @click="save">
            {{ status?.source === "managed" ? "替换证书" : "保存证书" }}
          </SButton>
          <SButton v-if="status?.source === 'managed'" type="error" :disabled="submitting" @click="requestTlsRemoval">删除证书</SButton>
        </SSpace>
      </SForm>
    </SCard>

    <SAlert type="warning">
      首次从 HTTP 配置证书时，提交请求本身仍未加密。请只在受信任网络中操作，并在保存后立即重启服务。
    </SAlert>
  </section>
</template>