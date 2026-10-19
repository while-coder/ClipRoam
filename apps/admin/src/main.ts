import { createApp } from "vue";
import { displayName } from "./branding";
import { createUiKit } from "@qingfeng346/ui-kit";
import App from "./App.vue";
import { router } from "./router/index.js";
import "./styles.css";

document.title = `${displayName} 管理后台`;
createApp(App).use(router).use(createUiKit()).mount("#app");
