import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import { configureNative } from "@cliproam/ui/native";
import { NativeFirefox } from "./native";
import "@cliproam/ui/styles.css";
import "./styles.css";

async function start(): Promise<void> {
  configureNative(new NativeFirefox());
  const { default: App } = await import("./App.vue");
  createApp(App).use(createUiKit()).mount("#app");
}

void start();
