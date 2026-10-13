import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import App from "./App.vue";
import "@cliproam/ui/styles.css";

createApp(App).use(createUiKit()).mount("#app");
