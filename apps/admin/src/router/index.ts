import { createRouter, createWebHistory } from "vue-router";
import { authenticated, ensureAuthenticated } from "../shared/auth.js";
import LoginSection from "../sections/login/LoginSection.vue";
import OverviewSection from "../sections/overview/OverviewSection.vue";
import UsersSection from "../sections/users/UsersSection.vue";
import FilesSection from "../sections/files/FilesSection.vue";
import TransferSection from "../sections/transfer/TransferSection.vue";

// index.html sets the external admin root, including a reverse-proxy prefix.
export const router = createRouter({
  history: createWebHistory(new URL(document.baseURI).pathname),
  routes: [
    { path: "/login", name: "login", component: LoginSection, meta: { public: true } },
    { path: "/", name: "overview", component: OverviewSection },
    { path: "/users", name: "users", component: UsersSection },
    { path: "/files", name: "files", component: FilesSection },
    { path: "/transfer", name: "transfer", component: TransferSection },
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
