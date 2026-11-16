<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { native, FilePickerMode } from '@cliproam/ui/native';
import { showNotification } from '@cliproam/ui/platform';
import { Radio, FileUp, Send, Copy, Download } from 'lucide-vue-next';
import type { LanSnapshot, LanConnection, LanRecord } from './types';
const visible = ref(false);
defineExpose({ open: () => { visible.value = true; } });
const snapshot = ref<LanSnapshot>({ running: false, addresses: [], peers: [], incoming: [], records: [] });
const name = ref('');
const target = ref('');
const connection = ref<LanConnection>();
const text = ref('');
const error = ref('');
const busy = ref(false);
const connecting = ref(false);
const sending = ref(false);
const confirming = ref(false);
const exportBusy = ref('');
const active = computed(() => snapshot.value.records.some(r => r.status === 'transferring' || r.status === 'waiting'));
const incoming = computed(() => snapshot.value.incoming[0]);
let disposed = false;
let timer: ReturnType<typeof setTimeout> | undefined;
const preferenceKey = 'cliproam.lan.enabled';
const nameKey = 'cliproam.lan.name';
function message(e: unknown) { return e instanceof Error ? e.message : String(e); }
function bytes(n: number) { return n < 1024 ? `${n} B` : n < 1024 ** 2 ? `${(n / 1024).toFixed(1)} KB` : n < 1024 ** 3 ? `${(n / 1024 ** 2).toFixed(1)} MB` : `${(n / 1024 ** 3).toFixed(1)} GB`; }
function status(r: LanRecord) { return ({ waiting: '等待对方确认', transferring: '传输中', completed: '已完成', failed: '失败' })[r.status]; }
async function refresh() {
  try { snapshot.value = await invoke<LanSnapshot>('native_lan_snapshot'); }
  catch (e) { error.value = message(e); }
}
async function poll() {
  if (disposed) return;
  await refresh();
  if (!disposed) timer = setTimeout(poll, snapshot.value.running ? 1000 : 3000);
}
async function start() {
  busy.value = true; error.value = '';
  try {
    await invoke('native_lan_start', { name: name.value }); await refresh();
    try { localStorage.setItem(preferenceKey, 'true'); localStorage.setItem(nameKey, name.value); } catch { /* Optional preference. */ }
  } catch (e) { error.value = message(e); }
  finally { busy.value = false; }
}
async function stop() {
  busy.value = true; error.value = '';
  try {
    await invoke('native_lan_stop'); connection.value = undefined;
    try { localStorage.setItem(preferenceKey, 'false'); } catch { /* Optional preference. */ }
    await refresh();
  } catch (e) { error.value = message(e); }
  finally { busy.value = false; }
}
async function connect(address = target.value) {
  connecting.value = true; error.value = ''; connection.value = undefined;
  try { connection.value = await invoke<LanConnection>('native_lan_connect', { target: address }); target.value = address; }
  catch (e) { error.value = `连接失败：${message(e)}`; }
  finally { connecting.value = false; }
}
async function decide(accept: boolean) {
  const request = incoming.value; if (!request) return;
  confirming.value = true;
  try { await invoke('native_lan_decide', { id: request.id, accept }); await refresh(); }
  catch (e) { error.value = message(e); }
  finally { confirming.value = false; }
}
async function sendText() {
  if (!connection.value || !text.value || sending.value) return;
  const content = text.value;
  sending.value = true; error.value = '';
  try { await invoke('native_lan_send', { connectionId: connection.value.connectionId, text: content, path: null }); if (text.value === content) text.value = ''; }
  catch (e) { error.value = message(e); }
  finally { sending.value = false; await refresh(); }
}
async function sendFiles() {
  if (!connection.value || sending.value) return;
  const selection = native.pickFiles(FilePickerMode.File, 'lan');
  sending.value = true; error.value = '';
  const staged: string[] = [];
  try {
    const picked = await selection;
    const paths = [...(picked.paths ?? [])];
    const { appDir } = await native.getPaths();
    const importedRoot = `${appDir}/histories/lan/files/picked/`;
    for (const path of paths) {
      if (path.startsWith(importedRoot)) {
        const directory = path.slice(importedRoot.length).split('/')[0];
        if (directory) staged.push(`${importedRoot}${directory}`);
      }
    }
    for (const file of picked.files ?? []) {
      if (file.size > 20 * 1024 ** 3) throw new Error('单文件最多支持 20 GiB');
      await native.createDirectory(`${appDir}/lan/outbox`);
      const folder = await native.createDirectory(`${appDir}/lan/outbox`, crypto.randomUUID());
      const path = `${folder}/${file.name.replace(/[\\/:*?"<>|\x00-\x1f]/g, '_')}`;
      staged.push(folder);
      if (!file.size) await native.writeFile(path, new Uint8Array(), false);
      for (let offset = 0; offset < file.size; offset += 1024 * 1024) await native.writeFile(path, new Uint8Array(await file.slice(offset, offset + 1024 * 1024).arrayBuffer()), offset > 0);
      paths.push(path);
    }
    for (const path of paths) await invoke('native_lan_send', { connectionId: connection.value.connectionId, path, text: null });
  } catch (e) { error.value = message(e); }
  finally {
    for (const path of staged) await native.removeFile(path).catch(() => undefined);
    sending.value = false; await refresh();
  }
}
async function copy(content: string) {
  try { await native.writeClipboard({ kind: 'text', text: content }, false); showNotification({ message: '已复制', tone: 'success' }); }
  catch (e) { error.value = message(e); }
}
async function copyRecord(record: LanRecord) {
  if (!record.path) return;
  try { await copy(new TextDecoder().decode(await native.readFile(record.path, 0, record.size))); }
  catch (e) { error.value = message(e); }
}
async function save(record: LanRecord) {
  if (!record.path || exportBusy.value) return;
  exportBusy.value = record.id;
  try {
    const destination = await native.selectSaveTarget(true, record.name);
    if (!destination) return;
    await native.copyFile(record.path, destination.value, destination.kind === 'documentTree' ? record.name : undefined);
    showNotification({ message: '已保存', tone: 'success' });
  } catch (e) { error.value = message(e); }
  finally { exportBusy.value = ''; }
}
onMounted(async () => {
  try { const device = await native.getDeviceInfo(); name.value = device.deviceAlias || device.deviceName || 'ClipRoam'; }
  catch { name.value = 'ClipRoam'; }
  await refresh();
  let autoStart = false;
  try { autoStart = localStorage.getItem(preferenceKey) === 'true'; name.value = localStorage.getItem(nameKey) || name.value; } catch { /* Optional preference. */ }
  if (!disposed && autoStart && !snapshot.value.running) await start();
  if (!disposed) void poll();
});
onBeforeUnmount(() => { disposed = true; if (timer) clearTimeout(timer); });
</script>

<template>
  <SModal :show="visible" title="局域网传输" width="760px" @update:show="visible = $event">
    <div class="lan-content">
      <p class="hint">无需登录，文本和文件直接发送到同一局域网的设备。手机传输时请保持 App 在前台。</p>
      <section class="lan-section">
        <div class="row between"><strong>本机<span v-if="active"> · 有传输任务</span></strong><SButton :disabled="busy" @click="snapshot.running ? stop() : start()">{{ snapshot.running ? '关闭局域网传输' : '开启局域网传输' }}</SButton></div>
        <label v-if="!snapshot.running" class="field">设备名称<input v-model="name" maxlength="40" placeholder="设备名称" :disabled="busy" /></label>
        <template v-else>
          <span>{{ snapshot.device?.name }}</span>
          <div class="row wrap"><SButton v-for="address in snapshot.addresses" :key="address" size="small" @click="copy(address)"><Copy :size="14" />{{ address }}</SButton></div>
          <p v-if="!snapshot.addresses.length" class="hint">未找到可用的 IPv4 地址，请检查网络连接。</p>
          <p class="hint">本机校验码：<code>{{ snapshot.device?.code }}</code>。接收前请与发送端显示的目标校验码核对。</p>
        </template>
      </section>
      <section v-if="snapshot.running" class="lan-section">
        <strong>发送到</strong>
        <p v-if="snapshot.discoveryError" class="hint">{{ snapshot.discoveryError }}</p>
        <div class="row wrap"><SButton v-for="peer in snapshot.peers" :key="peer.address" :disabled="connecting || sending" @click="connect(peer.address)"><Radio :size="14" />{{ peer.name }} · {{ peer.address }}</SButton></div>
        <p v-if="!snapshot.peers.length" class="hint">正在发现附近设备；也可以手动输入对方的 IP 和端口。请在对方 App 中开启局域网传输。</p>
        <form class="row" @submit.prevent="connect()"><input v-model="target" aria-label="对方 IP 和端口" placeholder="192.168.1.10:53319" :disabled="sending || connecting" /><SButton :disabled="connecting || sending || !target.trim()" @click="connect()">{{ connecting ? '连接中…' : '连接' }}</SButton></form>
        <template v-if="connection">
          <p>已连接 {{ connection.device.name }} · 目标校验码：<code>{{ connection.device.code }}</code></p>
          <p class="hint">请与接收端本机校验码核对一致，再发送内容。设备重启或重新开启后需要重新连接。</p>
          <label class="field">文本<textarea v-model="text" :disabled="sending" maxlength="250000" rows="4" placeholder="输入要发送的文本" /></label>
          <div class="row wrap"><SButton type="primary" :disabled="sending || !text" @click="sendText"><Send :size="15" />发送文本</SButton><SButton :disabled="sending" @click="sendFiles"><FileUp :size="15" />选择并发送文件</SButton><span v-if="sending" role="status" class="hint">等待接收或正在传输…</span></div>
        </template>
      </section>
      <p v-if="error" class="lan-error" role="alert">{{ error }}</p>
      <section v-if="snapshot.records.length" class="lan-section">
        <strong>传输记录</strong>
        <article v-for="record in snapshot.records" :key="record.id" class="record">
          <div class="row between"><strong>{{ record.name }}</strong><span>{{ status(record) }}</span></div>
          <span class="hint">{{ record.direction === 'send' ? '发送给' : '来自' }} {{ record.peer }} · {{ bytes(record.size) }}</span>
          <template v-if="record.status === 'transferring'"><progress :max="record.size || 1" :value="record.transferred" /><span class="hint">{{ bytes(record.transferred) }} / {{ bytes(record.size) }}</span></template>
          <p v-if="record.error" class="lan-error">{{ record.error }}</p>
          <template v-if="record.direction === 'receive' && record.status === 'completed'">
            <pre v-if="record.text !== null && record.text !== undefined">{{ record.text }}</pre>
            <div class="row wrap"><SButton v-if="record.text !== null && record.text !== undefined" size="small" @click="copyRecord(record)"><Copy :size="14" />复制文本</SButton><SButton size="small" :disabled="Boolean(exportBusy)" @click="save(record)"><Download :size="14" />另存为</SButton></div>
          </template>
        </article>
      </section>
      <p class="hint">支持文本与普通文件，多文件逐个确认；文件夹请先打包。接收内容仅保存在本机，不会自动上传或同步。</p>
    </div>
  </SModal>
  <SModal nested :show="Boolean(incoming)" title="接收局域网传输" width="480px" :closable="false" :mask-closable="false" :close-on-esc="false">
    <div v-if="incoming" class="lan-content">
      <p><strong>{{ incoming.offer.sender }}</strong>（{{ incoming.address }}）请求发送：</p>
      <p>{{ incoming.offer.kind === 'text' ? '文本' : incoming.offer.name }} · {{ bytes(incoming.offer.size) }}</p>
      <p>请确认发送端显示的目标校验码为 <code>{{ snapshot.device?.code }}</code>。</p>
      <p class="hint">接受后将保存到本机。请求在 2 分钟后过期。</p>
    </div>
    <template #footer><SButton :disabled="confirming" @click="decide(false)">拒绝</SButton><SButton type="primary" :disabled="confirming" @click="decide(true)">核对并接收</SButton></template>
  </SModal>
</template>

<style scoped>
.lan-content, .lan-section, .record, .field { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.lan-content { font-size: 14px; line-height: 1.6; }
.lan-content p { margin: 0; overflow-wrap: anywhere; }
.lan-section { padding: 16px 0; border-bottom: 1px solid var(--sui-border, #ddd); }
.row { display: flex; align-items: center; gap: 10px; min-width: 0; }
.between { justify-content: space-between; }
.wrap { flex-wrap: wrap; }
.hint { color: var(--sui-fg-secondary); font-size: 12px; }
input, textarea { box-sizing: border-box; width: 100%; min-width: 0; border: 1px solid var(--sui-border, #ccc); border-radius: 8px; padding: 10px 12px; color: inherit; background: var(--sui-bg, transparent); font: inherit; }
textarea { resize: vertical; }
.lan-error { color: var(--sui-danger, #c33); overflow-wrap: anywhere; }
.record { padding: 12px; background: var(--sui-bg-secondary, rgba(128,128,128,.06)); border-radius: 8px; }
progress { width: 100%; }
pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 180px; overflow: auto; margin: 0; font: inherit; }
code { font-size: 12px; overflow-wrap: anywhere; user-select: text; }
@media (max-width: 600px) { .between { flex-wrap: wrap; } }
</style>
