<script setup lang="ts">
import { onMounted, ref } from "vue";
import { fetchStatus, fetchUsers, type TlsStatus } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

const tls = ref<TlsStatus>();
const userCount = ref(0);
const error = ref("");

const tlsSummary = ref("加载中…");

onMounted(async () => {
  try {
    const [status, users] = await Promise.all([fetchStatus(), fetchUsers()]);
    tls.value = status.tls;
    userCount.value = users.length;
    tlsSummary.value = status.tls.enabled ? "HTTPS 已启用（后台管理）" : "尚未启用 HTTPS";
  } catch (reason) {
    error.value = errorMessage(reason, "加载状态失败。");
  }
});
</script>

<template>
  <section class="content-section" aria-labelledby="overview-title">
    <header class="topbar">
      <h1 id="overview-title">概览</h1>
    </header>

    <p v-if="error" class="message error" role="alert">{{ error }}</p>

    <section class="status-card" aria-labelledby="tls-status-title">
      <div class="status-copy">
        <p id="tls-status-title" class="label">TLS 状态</p>
        <strong>{{ tlsSummary }}</strong>
      </div>
      <span class="status-pill" :class="{ active: tls?.enabled }">{{ tls?.enabled ? "已启用" : "未启用" }}</span>
    </section>

    <section class="status-card" aria-labelledby="user-count-title">
      <div class="status-copy">
        <p id="user-count-title" class="label">注册用户</p>
        <strong>{{ userCount }} 位用户</strong>
      </div>
      <span class="status-pill">{{ userCount > 0 ? "使用中" : "暂无" }}</span>
    </section>
  </section>
</template>
