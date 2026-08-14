<script setup lang="ts">
import { computed, onMounted, ref } from "vue";

type TlsStatus = { enabled: boolean; source: "environment" | "managed" | "none" };
type TransferSettings = { maxStoredFileMb: number; resumableUploadTtlHours: number };
type StatusResponse = { tls: TlsStatus; transfer: TransferSettings };

const authenticated = ref(false);
const loading = ref(true);
const submitting = ref(false);
const password = ref("");
const certificate = ref("");
const privateKey = ref("");
const error = ref("");
const notice = ref("");
const status = ref<TlsStatus>();
const maxStoredFileMb = ref(100);
const resumableUploadTtlHours = ref(24);
const confirmingTlsRemoval = ref(false);

const tlsSummary = computed(() => {
  if (!status.value?.enabled) return "尚未启用 HTTPS";
  return status.value.source === "environment" ? "HTTPS 已启用（环境变量管理）" : "HTTPS 已启用（后台管理）";
});

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/admin/api/${path}`, {
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", ...init?.headers },
    ...init,
  });
  const body = await response.json().catch(() => ({})) as { message?: string } & T;
  if (!response.ok) throw new Error(body.message ?? `请求失败（${response.status}）`);
  return body;
}

async function loadStatus(): Promise<void> {
  try {
    const result = await request<StatusResponse>("status");
    status.value = result.tls;
    maxStoredFileMb.value = result.transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = result.transfer.resumableUploadTtlHours;
    authenticated.value = true;
  } catch {
    authenticated.value = false;
  } finally {
    loading.value = false;
  }
}

async function login(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  try {
    await request("login", { method: "POST", body: JSON.stringify({ password: password.value }) });
    password.value = "";
    authenticated.value = true;
    await loadStatus();
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "登录失败。";
  } finally {
    submitting.value = false;
  }
}

async function saveTls(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await request<{ tls: TlsStatus; restartRequired: boolean }>("tls", {
      method: "PUT",
      body: JSON.stringify({ cert: certificate.value, key: privateKey.value }),
    });
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

async function removeTls(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await request<{ tls: TlsStatus; restartRequired: boolean }>("tls", { method: "DELETE" });
    status.value = result.tls;
    confirmingTlsRemoval.value = false;
    notice.value = "证书已删除。服务仍会维持当前 HTTPS 直到重启；重启后同一端口将回到 HTTP/WS。";
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "删除证书失败。";
  } finally {
    submitting.value = false;
  }
}

async function saveTransferSettings(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  notice.value = "";
  try {
    const result = await request<{ transfer: TransferSettings }>("transfer-settings", {
      method: "PUT",
      body: JSON.stringify({
        maxStoredFileMb: maxStoredFileMb.value,
        resumableUploadTtlHours: resumableUploadTtlHours.value,
      }),
    });
    maxStoredFileMb.value = result.transfer.maxStoredFileMb;
    resumableUploadTtlHours.value = result.transfer.resumableUploadTtlHours;
    notice.value = "传输设置已保存，并已应用到后续上传与续传。";
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : "保存传输设置失败。";
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

async function logout(): Promise<void> {
  await request("logout", { method: "POST" }).catch(() => undefined);
  authenticated.value = false;
  status.value = undefined;
  certificate.value = "";
  privateKey.value = "";
  notice.value = "";
  confirmingTlsRemoval.value = false;
}

onMounted(loadStatus);
</script>

<template>
  <main class="page-shell">
    <section v-if="loading" class="loading" aria-live="polite">正在加载管理后台…</section>

    <section v-else-if="!authenticated" class="login-card" aria-labelledby="login-title">
      <p class="eyebrow">CLIPROAM ADMIN</p>
      <h1 id="login-title">管理后台</h1>
      <p class="muted">使用服务端配置的管理员密码登录。</p>
      <form @submit.prevent="login">
        <label for="admin-password">管理员密码</label>
        <input id="admin-password" v-model="password" type="password" autocomplete="current-password" :disabled="submitting" required autofocus />
        <p v-if="error" class="message error" role="alert">{{ error }}</p>
        <button type="submit" :disabled="submitting">{{ submitting ? "正在验证…" : "登录" }}</button>
      </form>
      <p class="footnote">正式服务需设置非空的 <code>CLIPROAM_ADMIN_PASSWORD</code>。</p>
    </section>

    <section v-else class="dashboard" aria-labelledby="dashboard-title">
      <header class="topbar">
        <div>
          <p class="eyebrow">CLIPROAM ADMIN</p>
          <h1 id="dashboard-title">服务安全设置</h1>
        </div>
        <button class="secondary" type="button" @click="logout">退出登录</button>
      </header>

      <section class="status-card" aria-labelledby="tls-status-title">
        <div class="status-copy">
          <p id="tls-status-title" class="label">TLS 状态</p>
          <strong>{{ tlsSummary }}</strong>
        </div>
        <span class="status-pill" :class="{ active: status?.enabled }">{{ status?.enabled ? "已启用" : "未启用" }}</span>
      </section>

      <section class="panel" aria-labelledby="transfer-settings-title">
        <div class="panel-heading">
          <div>
            <h2 id="transfer-settings-title">文件传输</h2>
            <p>修改后立即影响新上传与下一次断点续传，不会中断正在传输的文件。</p>
          </div>
        </div>

        <form @submit.prevent="saveTransferSettings">
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

      <section class="panel" aria-labelledby="tls-settings-title">
        <div class="panel-heading">
          <div>
            <h2 id="tls-settings-title">HTTPS 证书</h2>
            <p>上传 PEM 格式证书链与对应的私钥。私钥不会在后台再次显示。</p>
          </div>
          <span v-if="status?.source === 'environment'" class="readonly">环境变量管理</span>
        </div>

        <form @submit.prevent="saveTls">
          <label for="certificate">证书或完整证书链</label>
          <input id="certificate-file" class="file-input" type="file" accept=".pem,.crt,.cer,text/plain" @change="loadPem($event, 'certificate')" />
          <textarea id="certificate" v-model="certificate" rows="7" spellcheck="false" placeholder="-----BEGIN CERTIFICATE-----" :disabled="submitting || status?.source === 'environment'" required />

          <label for="private-key">私钥</label>
          <input id="private-key-file" class="file-input" type="file" accept=".pem,.key,text/plain" @change="loadPem($event, 'privateKey')" />
          <textarea id="private-key" v-model="privateKey" rows="7" spellcheck="false" placeholder="-----BEGIN PRIVATE KEY-----" :disabled="submitting || status?.source === 'environment'" required />

          <p v-if="error" class="message error" role="alert">{{ error }}</p>
          <p v-if="notice" class="message success" role="status">{{ notice }}</p>
          <div class="form-actions">
            <button type="submit" :disabled="submitting || status?.source === 'environment'">
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
            <button class="danger" type="button" :disabled="submitting" @click="removeTls">{{ submitting ? "正在删除…" : "确认删除" }}</button>
          </div>
        </section>
      </div>

      <p class="warning">首次从 HTTP 配置证书时，提交请求本身仍未加密。请只在受信任网络中操作，并在保存后立即重启服务。</p>
    </section>
  </main>
</template>
