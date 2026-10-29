<script setup lang="ts">
import { computed, ref } from "vue";
import { MAX_FAVORITE_FOLDER_DEPTH, type FavoriteFolder } from "@cliproam/protocol";
import { getAccountSession } from "../../accountSession";
import { errorMessage } from "../../utils";
import FavoriteFolderSelect from "./FavoriteFolderSelect.vue";
import { flattenFavoriteFolders } from "./folderTree";
const props = defineProps<{ folders: FavoriteFolder[]; initialFolderId: string | null; count: number; busy: boolean; error: string; loading: boolean }>();
const emit = defineEmits<{ close: []; confirm: [folderId: string | null]; refresh: [] }>();
const folderId = ref(props.initialFolderId);
const creating = ref(false);
const name = ref("");
const parentId = ref<string | null>(null);
const savingFolder = ref(false);
const folderError = ref("");
const allowedParents = computed(() => flattenFavoriteFolders(props.folders).filter((folder) => folder.depth < MAX_FAVORITE_FOLDER_DEPTH).map((folder) => folder.id));
async function createFolder(): Promise<void> {
  if (savingFolder.value || !name.value.trim()) return;
  const session = getAccountSession();
  if (!session) return;
  savingFolder.value = true;
  folderError.value = "";
  try {
    const value = name.value.trim();
    await session.history.saveFavoriteFolder({ name: value, parentId: parentId.value });
    const latest = await session.history.listFavoriteFolders();
    if (session.signal.aborted) return;
    folderId.value = latest.find((folder) => folder.name === value && folder.parentId === parentId.value)?.id ?? null;
    creating.value = false;
    name.value = "";
    emit("refresh");
  } catch (cause) { if (!session.signal.aborted) folderError.value = errorMessage(cause); }
  finally { savingFolder.value = false; }
}
</script>

<template>
  <SModal :show="true" :title="`收藏到目录 · ${count} 条`" width="460px" :closable="!busy && !savingFolder" :mask-closable="!busy && !savingFolder" :close-on-esc="!busy && !savingFolder" @update:show="!busy && !savingFolder && emit('close')">
    <div class="destination-form">
      <label>目标目录<FavoriteFolderSelect v-model="folderId" :folders="folders" :disabled="busy || savingFolder || loading" /></label>
      <p>已收藏的内容会移动到所选目录。</p>
      <SButton v-if="!creating" :disabled="busy || loading" @click="creating = true">新建目录</SButton>
      <form v-else class="inline-folder-form" @submit.prevent="createFolder">
        <label>目录名称<SInput v-model:value="name" maxlength="80" :disabled="savingFolder" /></label>
        <label>父目录<FavoriteFolderSelect v-model="parentId" :folders="folders" :allowed-ids="allowedParents" null-label="无（一级目录）" :disabled="savingFolder" /></label>
        <div><SButton :disabled="savingFolder" @click="creating = false">取消新建</SButton><SButton :loading="savingFolder" :disabled="!name.trim()" @click="createFolder">创建并选中</SButton></div>
      </form>
      <p v-if="loading" role="status">正在读取目录…</p>
      <p v-if="folderError || error" role="alert">{{ folderError || error }}</p>
    </div>
    <template #footer><SButton :disabled="busy || savingFolder" @click="emit('close')">取消</SButton><SButton type="primary" :loading="busy" :disabled="loading || savingFolder || creating" @click="emit('confirm', folderId)">确定</SButton></template>
  </SModal>
</template>

<style scoped>
.destination-form, label, .inline-folder-form { display: grid; gap: 8px; }
.destination-form { gap: 16px; }
.inline-folder-form { padding: 12px; border: 1px solid var(--color-border); border-radius: 6px; gap: 12px; }
.inline-folder-form > div { display: flex; gap: 8px; }
p { margin: 0; color: var(--color-secondary-text); font-size: 12px; }
</style>
