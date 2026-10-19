<script setup lang="ts">
import SharedApp from "@cliproam/ui/app";
import { getAccountSession } from "@cliproam/ui/account-session";
import { onMounted, onBeforeUnmount } from "vue";
import { postMessage } from "./bridge";

async function capture(text: string): Promise<void> {
  const session = getAccountSession();
  if (!session) throw new Error("请先登录 随心贴，再保存选中文本");
  if (!text.trim()) throw new Error("请在编辑器中选中要保存的文本");
  await session.capture.capture({ kind: "text", text });
}

async function receive({ data }: MessageEvent): Promise<void> {
  if (data?.type === "captureSelection" && typeof data.text === "string" && typeof data.id === "number") {
    try { await capture(data.text); postMessage({ type: "captured", id: data.id }); }
    catch (error) { postMessage({ type: "captured", id: data.id, error: error instanceof Error ? error.message : String(error) }); }
  }
}

onMounted(() => { window.addEventListener("message", receive); postMessage({ type: "ready" }); });
onBeforeUnmount(() => window.removeEventListener("message", receive));
</script>

<template><SharedApp /></template>
