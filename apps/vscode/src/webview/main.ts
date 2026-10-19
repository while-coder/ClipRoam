import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import { configureNative } from "@cliproam/ui/native";
import { configureAppLocale } from "@cliproam/ui/branding";
import { NativeVscode } from "./native";
import { configureShareReceiver } from "@cliproam/ui/share";
import { VscodeShareReceiver } from "./share";
import "@cliproam/ui/styles.css";
import "./styles.css";

async function start(): Promise<void> {
  const native = new NativeVscode();
  configureNative(native);
  try {
    configureAppLocale((await native.getDeviceInfo()).systemLocale);
  } catch (error) {
    console.warn("Could not read VS Code locale; using browser locale", error);
  }
  configureShareReceiver(new VscodeShareReceiver());
  const { default: App } = await import("./App.vue");
  createApp(App).use(createUiKit()).mount("#app");
}
void start();
