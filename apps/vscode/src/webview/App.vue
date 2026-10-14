<script setup lang="ts">
import SharedApp from "@cliproam/ui/app";
import { getAccountSession } from "@cliproam/ui/account-session";
import { onMounted, onBeforeUnmount, ref } from "vue";
import { copiedText } from "./native";
import { postMessage, request } from "./bridge";

const busy = ref(false);
const message = ref("");
const failed = ref(false);

async function capture(text: string): Promise<void> {
  const session = getAccountSession();
  if (!session) throw new Error("请先登录 ClipRoam，再保存选中文本");
  if (!text.trim()) throw new Error("请在编辑器中选中要保存的文本");
  await session.capture.capture({ kind: "text", text });
}

async function run(action: () => Promise<void>, success: string): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  message.value = "";
  try { await action(); failed.value = false; message.value = success; }
  catch (error) { failed.value = true; message.value = error instanceof Error ? error.message : String(error); }
  finally { busy.value = false; }
}

function saveSelection(): void {
  void run(async () => capture(await request<string>("editor.selection")), "已加入上传队列");
}
function insertText(): void {
  const text = copiedText.value;
  if (text !== undefined) void run(async () => { await request("editor.insert", text); }, "已插入编辑器");
}

async function receive({ data }: MessageEvent): Promise<void> {
  if (data?.type === "insertHistory") {
    failed.value = false;
    message.value = "双击历史文本或使用复制菜单，然后点击“插入已复制文本”。";
  }
  if (data?.type === "captureSelection" && typeof data.text === "string" && typeof data.id === "number") {
    try { await capture(data.text); postMessage({ type: "captured", id: data.id }); }
    catch (error) { postMessage({ type: "captured", id: data.id, error: error instanceof Error ? error.message : String(error) }); }
  }
}

onMounted(() => { window.addEventListener("message", receive); postMessage({ type: "ready" }); });
onBeforeUnmount(() => window.removeEventListener("message", receive));
</script>

<template>
  <div class="vscode-shell">
    <header class="vscode-toolbar" aria-label="编辑器操作">
      <button type="button" :disabled="busy" @click="saveSelection">保存编辑器选中文本</button>
      <button type="button" :disabled="busy || copiedText === undefined" @click="insertText">插入已复制文本</button>
      <span role="status" :class="{ failed }">{{ busy ? '正在处理…' : message || '双击历史文本复制，再插入编辑器。' }}</span>
    </header>
    <div class="vscode-content"><SharedApp /></div>
  </div>
</template>
