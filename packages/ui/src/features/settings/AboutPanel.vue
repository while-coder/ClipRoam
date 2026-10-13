<script setup lang="ts">
import { BookOpen, ExternalLink, Github, MessageSquare, Package } from "lucide-vue-next";
import { computed, ref } from "vue";
import { native } from "../../platform/native";
import { useAppVersion } from "../../platform/runtime";
import { errorMessage } from "../../utils";
import productIcon from "../../assets/cliproam-icon.png";

const projectUrl = "https://github.com/while-coder/ClipRoam";
const { appVersion } = useAppVersion();
const linkError = ref("");
const feedbackUrl = computed(() => {
  const url = new URL(`${projectUrl}/issues/new`);
  url.searchParams.set("body", [
    "### 使用环境",
    `- ClipRoam 版本：${appVersion.value || "未知"}`,
    "- 操作系统及版本：",
    "",
    "### 问题或建议",
    "请描述遇到的问题或希望增加的功能。",
    "",
    "### 复现步骤",
    "1. ",
    "2. ",
    "",
    "### 预期结果与实际表现",
    "",
  ].join("\n"));
  return url.href;
});

async function openLink(url: string): Promise<void> {
  linkError.value = "";
  try {
    await native.openExternalUrl(url);
  } catch (error) {
    linkError.value = `无法打开链接：${errorMessage(error)}`;
  }
}
</script>

<template>
  <SCard class="about-product">
    <img class="about-product-mark" :src="productIcon" alt="" />
    <div class="about-product-copy">
      <h4>ClipRoam</h4>
      <p>保存每一次复制，让文字、图片和文件在你的设备之间漫游。</p>
    </div>
    <span class="about-version">v{{ appVersion || "…" }}</span>
    <div class="about-features" aria-label="应用功能">
      <STag size="small">剪贴板历史</STag>
      <STag size="small">跨设备同步</STag>
      <STag size="small">自托管服务</STag>
    </div>
  </SCard>

  <slot />

  <SCard class="about-community" title="项目与支持">
    <a class="about-repository" :href="projectUrl" target="_blank" rel="noopener noreferrer" @click.prevent="openLink(projectUrl)">
      <Github :size="20" aria-hidden="true" />
      <span>
        <strong>GitHub 项目</strong>
        <span>github.com/while-coder/ClipRoam</span>
      </span>
      <ExternalLink :size="14" aria-hidden="true" />
    </a>
    <p class="settings-section-desc">查看源码、部署说明与最新进展，欢迎分享建议和参与贡献。</p>
    <div class="about-links">
      <SButton @click="openLink(`${projectUrl}#readme`)">
        <BookOpen :size="16" aria-hidden="true" />使用指南
      </SButton>
      <SButton @click="openLink(`${projectUrl}/releases`)">
        <Package :size="16" aria-hidden="true" />版本发布
      </SButton>
      <SButton type="primary" @click="openLink(feedbackUrl)">
        <MessageSquare :size="16" aria-hidden="true" />问题与建议
      </SButton>
    </div>
    <p class="about-feedback-hint">反馈将在 GitHub 中填写，需登录 GitHub 账号。</p>
    <SAlert v-if="linkError" type="error" role="alert">{{ linkError }}</SAlert>
  </SCard>
</template>
