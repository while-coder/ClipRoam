import { createRouter, createWebHistory } from "vue-router";
import { authenticated, ensureAuthenticated } from "../shared/auth.js";
import LoginSection from "../sections/login/LoginSection.vue";
import OverviewSection from "../sections/overview/OverviewSection.vue";
import UsersSection from "../sections/users/UsersSection.vue";
import FilesSection from "../sections/files/FilesSection.vue";
import TransferSection from "../sections/transfer/TransferSection.vue";
import SecuritySection from "../sections/security/SecuritySection.vue";

// The admin UI is mounted under /admin/ and the server falls back to
// index.html for extension-less /admin/* paths, so history mode deep links work.
export const router = createRouter({
  history: createWebHistory("/admin/"),
  routes: [
    { path: "/login", name: "login", component: LoginSection, meta: { public: true } },
    { path: "/", name: "overview", component: OverviewSection },
    { path: "/users", name: "users", component: UsersSection },
    { path: "/files", name: "files", component: FilesSection },
    { path: "/transfer", name: "transfer", component: TransferSection },
    { path: "/security", name: "security", component: SecuritySection },
    { path: "/:pathMatch(.*)*", redirect: "/" },
  ],
});

router.beforeEach(async (to) => {
  if (to.meta.public) {
    return authenticated.value ? { path: "/" } : true;
  }
  if (!(await ensureAuthenticated())) {
    return { name: "login", query: { redirect: to.fullPath } };
  }
  return true;
});
