import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import App from "./App.vue";
import { router } from "./router/index.js";
import "./styles.css";

createApp(App).use(router).use(createUiKit()).mount("#app");
