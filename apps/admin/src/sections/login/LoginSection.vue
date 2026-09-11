<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { login as submitLogin } from "../../shared/api.js";
import { errorMessage } from "../../shared/errorMessage.js";
import { markAuthenticated } from "../../shared/auth.js";

const route = useRoute();
const router = useRouter();

const password = ref("");
const submitting = ref(false);
const error = ref("");

async function login(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  error.value = "";
  try {
    await submitLogin(password.value);
    password.value = "";
    markAuthenticated();
    const redirect = typeof route.query.redirect === "string" ? route.query.redirect : "/";
    await router.push(redirect);
  } catch (reason) {
    error.value = errorMessage(reason, "登录失败。");
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <section class="login-card" aria-labelledby="login-title">
    <p class="eyebrow">CLIPROAM ADMIN</p>
    <h1 id="login-title">管理后台</h1>
    <p class="muted">使用服务端配置的管理员密码登录。</p>
    <form @submit.prevent="login">
      <label for="admin-password">管理员密码</label>
      <input id="admin-password" v-model="password" type="password" autocomplete="current-password" :disabled="submitting" required autofocus />
      <p v-if="error" class="message error" role="alert">{{ error }}</p>
      <button type="submit" :disabled="submitting">{{ submitting ? "正在验证…" : "登录" }}</button>
    </form>
    <p class="footnote">正式服务需设置非空的 <code>CLIPROAM_ADMIN_PASSWORD</code>。</p>
  </section>
</template>
