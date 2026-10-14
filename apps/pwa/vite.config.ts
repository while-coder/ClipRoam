import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  base: process.env.PWA_BASE ?? "./",
  server: {port:1431,strictPort:true},
});
