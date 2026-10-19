<script setup lang="ts">
import TransferSettingsForm from "./TransferSettingsForm.vue";
</script>

<template>
  <section class="content-section" aria-labelledby="transfer-title">
    <header class="topbar">
      <h1 id="transfer-title">文件传输</h1>
    </header>
    <SCard title="全局默认传输设置">
      <p class="muted panel-desc">各账号选择“全局配置”的项目会跟随以下设置更新。可在用户管理中逐项指定独立值。</p>
      <TransferSettingsForm />
    </SCard>
  </section>
</template>
