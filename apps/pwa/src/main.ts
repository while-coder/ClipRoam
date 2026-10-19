import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import { configureNative } from "@cliproam/ui/native";
import { NativePwa } from "./native";
import "@cliproam/ui/styles.css";

async function start(): Promise<void> {
  configureNative(new NativePwa());
  const { default: App } = await import("./App.vue");
  createApp(App).use(createUiKit()).mount("#app");
}

void start();

if (import.meta.env.PROD && "serviceWorker" in navigator) {
  void navigator.serviceWorker.register(`${import.meta.env.BASE_URL}sw.js`, {type:"module"})
    .catch((error) => console.error("系统分享服务注册失败", error));
}
