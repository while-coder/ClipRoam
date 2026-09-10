import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  base: "/admin/",
  plugins: [vue()],
  server: {
    // Dev server proxies the admin API (and serves the UI under the same
    // /admin/ base) against a locally running backend.
    proxy: {
      "/admin-api": "http://localhost:4810",
    },
  },
  build: {
    outDir: "../server/admin",
    emptyOutDir: true,
  },
});
