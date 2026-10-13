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
