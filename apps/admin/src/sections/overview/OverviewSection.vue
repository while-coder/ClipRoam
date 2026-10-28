<script setup lang="ts">
import { onMounted, ref } from "vue";
import { fetchUsers } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";

const userCount = ref(0);
const error = ref("");

onMounted(async () => {
  try {
    const users = await fetchUsers();
    userCount.value = users.length;
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

    <SAlert v-if="error" type="error">{{ error }}</SAlert>

    <SCard aria-labelledby="user-count-title">
      <div class="status-row">
        <div class="status-copy">
          <p id="user-count-title" class="label">注册用户</p>
          <strong>{{ userCount }} 位用户</strong>
        </div>
        <STag type="default">{{ userCount > 0 ? "使用中" : "暂无" }}</STag>
      </div>
    </SCard>
  </section>
</template>
