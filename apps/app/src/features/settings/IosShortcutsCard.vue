<script setup lang="ts">
import { appName } from "@cliproam/ui/branding";
import {
  iosShortcutsAvailable, iosShortcutsReady, iosShortcutsError, reconfigureIosShortcuts,
  iosQuickAddSupported, iosQuickAddConfigured, iosQuickAdding, iosQuickAddError, quickAddIosSmsShortcut,
} from "../shortcuts/iosShortcuts";

defineProps<{ currentUsername: string }>();
</script>

<template>
  <SCard class="ios-shortcuts-card" v-if="iosShortcutsAvailable" title="短信自动同步">
    <template v-if="iosQuickAddSupported">
      <SButton type="primary" :loading="iosQuickAdding" :disabled="!iosQuickAddConfigured || !iosShortcutsReady || iosQuickAdding" @click="quickAddIosSmsShortcut">快速添加短信自动同步</SButton>
      <p v-if="iosQuickAddConfigured" class="settings-section-desc">确认添加后，请在快捷指令中开启“自动化”，关闭“运行前确认”，并完成首次运行授权。</p>
      <p v-else class="settings-section-desc">快速添加暂不可用，可按下方步骤手动配置。</p>
      <p v-if="iosQuickAddError" class="settings-section-desc" role="alert">{{ iosQuickAddError }}</p>
    </template>
    <p class="settings-section-desc">在系统“快捷指令”中创建“收到信息”自动化，选择“立即运行”，添加“同步文本到 {{ appName }}”，将“文本”设为“快捷指令输入”（信息正文）。本动作不筛选验证码。</p>
    <p class="settings-section-desc">接收范围由系统自动化的发件人或信息包含条件决定。同步成功后，短信正文会显示在同账号设备的剪贴板历史中，无需打开 {{ appName }}。</p>
    <p v-if="iosShortcutsError" class="settings-section-desc" role="alert">快捷指令配置失败：{{ iosShortcutsError }}</p>
    <p v-else class="settings-section-desc" role="status">{{ iosShortcutsReady ? '账号已就绪，可在快捷指令中使用。' : `请先登录 ${appName}，登录后自动配置。` }}</p>
    <p class="settings-section-desc">快捷指令还提供“同步文件到 {{ appName }}”，可上传多文件及文件夹，图片可作为普通文件上传。</p>
    <p class="settings-section-desc">使用仅允许上传的独立凭证，保存在 iOS Keychain；不会读取短信列表。首次运行请解锁手机并完成系统授权，断网或凭证失效时会报告失败。</p>
    <SButton :disabled="!currentUsername || (!iosShortcutsReady && !iosShortcutsError)" @click="reconfigureIosShortcuts">重新配置</SButton>
  </SCard>
</template>

<style scoped>
.ios-shortcuts-card :deep(.s-card-body) { display: grid; gap: 12px; }
.settings-section-desc { margin: 0; color: var(--sui-fg-secondary); font-size: 14px; line-height: 1.6; overflow-wrap: anywhere; }
</style>
