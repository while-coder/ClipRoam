import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import { configureNative } from "@cliproam/ui/native";
import { NativeWeb } from "@cliproam/ui/native-web";
import { NativeRust } from "./platform/native_rust";
import { setupLogger } from "./utils/logger";
import "@cliproam/ui/styles.css";
import "./features/quick-paste/styles.css";

async function start(): Promise<void> {
  const platform = "__TAURI_INTERNALS__" in window ? new NativeRust() : new NativeWeb();
  configureNative(platform);
  setupLogger();
  const { default: App } = await (platform.windowLabel === "toast"
    ? import("./features/toast/ToastWindow.vue") : import("./App.vue"));
  createApp(App).use(createUiKit()).mount("#app");
}

void start();
