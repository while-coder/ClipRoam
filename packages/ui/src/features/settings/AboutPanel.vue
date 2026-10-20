<script setup lang="ts">
import { appName } from "../../branding";
import { BookOpen, ExternalLink, Github, MessageSquare, Package } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { native, type DeviceInfo } from "../../platform/native";
import { useAppVersion } from "../../platform/runtime";
import { errorMessage } from "../../utils";
import productIcon from "../../assets/cliproam-icon.png";

const projectUrl = "https://github.com/while-coder/ClipRoam";
const { appVersion } = useAppVersion();
const linkError = ref("");
const deviceInfo = ref<DeviceInfo>();
const openingFeedback = ref(false);
let deviceInfoRequest: Promise<void> | undefined;

function loadDeviceInfo(): Promise<void> {
  if (deviceInfo.value) return Promise.resolve();
  return deviceInfoRequest ??= native.getDeviceInfo().then((info) => {
    deviceInfo.value = info;
  }).catch(() => {
    // 信息读取失败也允许反馈，保留明确的补充提示；下次点击时重试。
    deviceInfoRequest = undefined;
  });
}

onMounted(() => { void loadDeviceInfo(); });

const operatingSystem = computed(() => {
  const info = deviceInfo.value;
  if (!info) return "未能读取，请补充";
  const names: Record<string, string> = {
    macos: "macOS", windows: "Windows", linux: "Linux",
    android: "Android", ios: "iOS", browser: "浏览器",
  };
  return [names[info.osType] || info.osType, info.osVersion].filter(Boolean).join(" ") || "未能读取，请补充";
});

const feedbackUrl = computed(() => {
  const url = new URL(`${projectUrl}/issues/new`);
  url.searchParams.set("body", [
    "### 使用环境",
    `- ${appName.value} 版本：${appVersion.value || deviceInfo.value?.appVersion || "未知"}`,
    `- 系统 / 浏览器及版本：${operatingSystem.value}`,
    ...(deviceInfo.value?.deviceBrand ? [`- 设备品牌：${deviceInfo.value.deviceBrand}`] : []),
    ...(deviceInfo.value?.deviceMarketName ? [`- 商品型号：${deviceInfo.value.deviceMarketName}`] : []),
    ...(deviceInfo.value?.deviceModel ? [`- 系统型号：${deviceInfo.value.deviceModel}`] : []),
    ...(deviceInfo.value?.androidApiLevel ? [`- Android API Level：${deviceInfo.value.androidApiLevel}`] : []),
    ...(deviceInfo.value?.appBuildNumber ? [`- 应用构建号：${deviceInfo.value.appBuildNumber}`] : []),
    ...(deviceInfo.value?.architecture ? [`- 处理器架构：${deviceInfo.value.architecture}`] : []),
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

async function openFeedback(): Promise<void> {
  if (openingFeedback.value) return;
  openingFeedback.value = true;
  try {
    await loadDeviceInfo();
    await openLink(feedbackUrl.value);
  } finally {
    openingFeedback.value = false;
  }
}

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
      <h4>{{ appName }}</h4>
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
      <SButton type="primary" :loading="openingFeedback" @click="openFeedback">
        <MessageSquare :size="16" aria-hidden="true" />问题与建议
      </SButton>
    </div>
    <p class="about-feedback-hint">反馈将在 GitHub 中填写，需登录 GitHub 账号。</p>
    <SAlert v-if="linkError" type="error" role="alert">{{ linkError }}</SAlert>
  </SCard>
</template>
