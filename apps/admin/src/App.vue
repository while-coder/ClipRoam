<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { logout } from "./shared/api.js";
import { markSignedOut } from "./shared/auth.js";

type MenuItem = { to: string; label: string; paths: string[] };

const menuItems: MenuItem[] = [
  { to: "/", label: "概览", paths: ["M3 10.5 12 3l9 7.5", "M5 9.5V21h14V9.5", "M10 21v-6h4v6"] },
  {
    to: "/users",
    label: "用户管理",
    paths: [
      "M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z",
      "M2 21v-1a5 5 0 0 1 5-5h4a5 5 0 0 1 5 5v1",
      "M17 3.5a4 4 0 0 1 0 7",
      "M19 15.5a5 5 0 0 1 3 4.5v1",
    ],
  },
  { to: "/files", label: "文件管理", paths: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z", "M14 2v6h6"] },
  { to: "/transfer", label: "文件传输", paths: ["M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4", "M17 8l-5-5-5 5", "M12 3v12"] },
  { to: "/security", label: "HTTPS 证书", paths: ["M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z"] },
];

const signOutPaths = ["M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4", "M16 17l5-5-5-5", "M21 12H9"];

const route = useRoute();
const router = useRouter();
const collapsed = ref(false);

async function signOut(): Promise<void> {
  await logout();
  markSignedOut();
  collapsed.value = false;
  await router.push({ name: "login" });
}
</script>

<template>
  <main v-if="route.meta.public" class="page-shell">
    <RouterView />
  </main>

  <div v-else class="layout" :class="{ collapsed }">
    <aside class="sidebar" aria-label="管理导航">
      <p class="eyebrow brand">CLIPROAM ADMIN</p>
      <nav class="menu">
        <RouterLink v-for="item in menuItems" :key="item.to" :to="item.to" class="menu-item" active-class="active">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path v-for="d in item.paths" :key="d" :d="d" /></svg>
          <span class="menu-label">{{ item.label }}</span>
        </RouterLink>
      </nav>
      <button class="menu-item signout" type="button" @click="signOut">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path v-for="d in signOutPaths" :key="d" :d="d" /></svg>
        <span class="menu-label">退出登录</span>
      </button>
      <button
        class="collapse-button"
        type="button"
        :aria-label="collapsed ? '展开菜单' : '收起菜单'"
        @click="collapsed = !collapsed"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 18l-6-6 6-6" /></svg>
      </button>
    </aside>

    <div class="content">
      <RouterView />
    </div>
  </div>
</template>
