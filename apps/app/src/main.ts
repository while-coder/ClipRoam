import { createApp } from "vue";
import App from "./App.vue";
import "./styles.css";
import { setupLogger } from "./utils/logger";

setupLogger();

createApp(App).mount("#app");
