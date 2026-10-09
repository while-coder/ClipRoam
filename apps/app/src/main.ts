import { createApp } from "vue";
import { createUiKit } from "@qingfeng346/ui-kit";
import App from "./App.vue";
import "./styles.css";
import { setupLogger } from "./utils/logger";

setupLogger();

createApp(App).use(createUiKit()).mount("#app");
