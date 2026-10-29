<script setup lang="ts">
import { computed, onDeactivated, ref } from "vue";
import { ChevronRight, Folder, FolderPlus, Pencil, Plus, Star, Trash2 } from "lucide-vue-next";
import { MAX_FAVORITE_FOLDER_DEPTH, type FavoriteFolder } from "@cliproam/protocol";
import { getAccountSession } from "../../accountSession";
import { useMobileLayout } from "../../platform/runtime";
import { errorMessage } from "../../utils";
import FavoriteFolderSelect from "./FavoriteFolderSelect.vue";
import { flattenFavoriteFolders } from "./folderTree";

const props = defineProps<{ folders: FavoriteFolder[]; modelValue: string; loading: boolean; error: string }>();
const emit = defineEmits<{ "update:modelValue": [value: string]; refresh: [] }>();
const mobile = useMobileLayout();
const rows = computed(() => flattenFavoriteFolders(props.folders));
const current = computed(() => rows.value.find((folder) => folder.id === props.modelValue));
const collapsed = ref(new Set<string>());
const visibleRows = computed(() => rows.value.filter((folder) => !rows.value.some((parent) => parent.id !== folder.id && collapsed.value.has(parent.id) && parent.descendantIds.includes(folder.id))));
const editorOpen = ref(false);
const editingId = ref<string>();
const name = ref("");
const parentId = ref<string | null>(null);
const deleteOpen = ref(false);
const deleteTarget = ref<typeof rows.value[number]>();
const busy = ref(false);
const formError = ref("");
onDeactivated(() => {
  if (!busy.value) { editorOpen.value = false; deleteOpen.value = false; }
});
const allowedParents = computed(() => {
  const editing = rows.value.find((folder) => folder.id === editingId.value);
  return rows.value.filter((folder) => folder.depth < MAX_FAVORITE_FOLDER_DEPTH &&
    !editing?.descendantIds.includes(folder.id) && (!editing || editing.descendantIds.length === 1)).map((folder) => folder.id);
});

function openEditor(edit: boolean, child = false): void {
  editingId.value = edit ? current.value?.id : undefined;
  name.value = edit ? current.value?.name ?? "" : "";
  parentId.value = edit ? current.value?.parentId ?? null : child ? current.value?.id ?? null : null;
  formError.value = "";
  editorOpen.value = true;
}
async function save(): Promise<void> {
  if (busy.value || !name.value.trim()) return;
  const session = getAccountSession();
  if (!session) return;
  busy.value = true;
  formError.value = "";
  try {
    await session.history.saveFavoriteFolder({ name: name.value.trim(), parentId: parentId.value }, editingId.value);
    if (session.signal.aborted) return;
    editorOpen.value = false;
    emit("refresh");
  } catch (error) { if (!session.signal.aborted) formError.value = errorMessage(error); }
  finally { busy.value = false; }
}
async function remove(): Promise<void> {
  if (busy.value || !deleteTarget.value) return;
  const session = getAccountSession();
  if (!session) return;
  busy.value = true;
  formError.value = "";
  try {
    await session.history.deleteFavoriteFolder(deleteTarget.value.id);
    if (session.signal.aborted) return;
    emit("update:modelValue", "unfiled");
    deleteOpen.value = false;
    emit("refresh");
  } catch (error) { if (!session.signal.aborted) formError.value = errorMessage(error); }
  finally { busy.value = false; }
}
</script>

<template>
  <aside class="favorite-folders" aria-label="收藏目录">
    <div v-if="mobile" class="folder-mobile-select">
      <label for="favorite-navigation">收藏目录</label>
      <select id="favorite-navigation" :value="modelValue" @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value)">
        <option value="">全部收藏</option>
        <option value="unfiled">未分类</option>
        <option v-for="folder in rows" :key="folder.id" :value="folder.id">{{ folder.path }}</option>
      </select>
    </div>
    <nav v-else class="folder-navigation" aria-label="收藏目录导航">
      <button class="folder-link" :class="{ active: !modelValue }" :aria-current="!modelValue ? 'page' : undefined" @click="emit('update:modelValue', '')"><Star :size="16" aria-hidden="true" />全部收藏</button>
      <button class="folder-link" :class="{ active: modelValue === 'unfiled' }" :aria-current="modelValue === 'unfiled' ? 'page' : undefined" @click="emit('update:modelValue', 'unfiled')"><Folder :size="16" aria-hidden="true" />未分类</button>
      <div v-for="folder in visibleRows" :key="folder.id" class="folder-row" :style="{ paddingLeft: `${(folder.depth - 1) * 16}px` }">
        <button v-if="folder.descendantIds.length > 1" class="folder-toggle" :aria-label="`${collapsed.has(folder.id) ? '展开' : '收起'}${folder.name}`" :aria-expanded="!collapsed.has(folder.id)"
          @click="collapsed.has(folder.id) ? collapsed.delete(folder.id) : collapsed.add(folder.id)"><ChevronRight :size="14" :class="{ expanded: !collapsed.has(folder.id) }" /></button>
        <span v-else class="folder-toggle-spacer" />
        <button class="folder-link" :class="{ active: modelValue === folder.id }" :aria-current="modelValue === folder.id ? 'page' : undefined" :title="folder.path" @click="emit('update:modelValue', folder.id)">
          <Folder :size="15" aria-hidden="true" /><span>{{ folder.name }}</span><small>{{ folder.count }}</small>
        </button>
      </div>
    </nav>
    <p v-if="loading" class="folder-status" role="status">正在读取目录…</p>
    <div v-if="error" class="folder-status" role="alert">{{ error }} <SButton size="small" @click="emit('refresh')">重试</SButton></div>
    <div class="folder-actions">
      <SButton size="small" :disabled="loading || !!error" @click="openEditor(false)"><Plus :size="14" aria-hidden="true" />新建目录</SButton>
      <template v-if="current">
        <SButton v-if="current.depth < MAX_FAVORITE_FOLDER_DEPTH" size="small" @click="openEditor(false, true)"><FolderPlus :size="14" aria-hidden="true" />子目录</SButton>
        <SButton size="small" @click="openEditor(true)"><Pencil :size="14" aria-hidden="true" />编辑</SButton>
        <SButton size="small" @click="formError = ''; deleteTarget = current; deleteOpen = true"><Trash2 :size="14" aria-hidden="true" />删除目录</SButton>
      </template>
    </div>
  </aside>

  <SModal v-if="editorOpen" :show="true" :title="editingId ? '编辑目录' : '新建目录'" width="440px" :closable="!busy" :mask-closable="!busy" :close-on-esc="!busy" @update:show="!busy && (editorOpen = false)">
    <form class="folder-form" @submit.prevent="save">
      <label>目录名称<SInput v-model:value="name" autofocus maxlength="80" :disabled="busy" /></label>
      <label>父目录<FavoriteFolderSelect v-model="parentId" :folders="folders" :allowed-ids="allowedParents" null-label="无（一级目录）" :disabled="busy" /></label>
      <p>最多支持 {{ MAX_FAVORITE_FOLDER_DEPTH }} 级目录。修改父目录即可移动目录。</p>
      <p v-if="formError" role="alert">{{ formError }}</p>
      <button type="submit" hidden />
    </form>
    <template #footer><SButton :disabled="busy" @click="editorOpen = false">取消</SButton><SButton type="primary" :loading="busy" :disabled="!name.trim()" @click="save">保存</SButton></template>
  </SModal>
  <SModal v-if="deleteOpen" :show="true" title="删除目录" width="440px" :closable="!busy" :mask-closable="!busy" :close-on-esc="!busy" @update:show="!busy && (deleteOpen = false)">
    <p>删除「{{ deleteTarget?.name }}」及其 {{ (deleteTarget?.descendantIds.length ?? 1) - 1 }} 个下级目录？其中 {{ deleteTarget?.count ?? 0 }} 条收藏会移到「未分类」，内容仍保持收藏。</p>
    <p v-if="formError" role="alert">{{ formError }}</p>
    <template #footer><SButton :disabled="busy" @click="deleteOpen = false">取消</SButton><SButton type="error" :loading="busy" @click="remove">删除目录，保留内容</SButton></template>
  </SModal>
</template>

<style scoped>
.favorite-folders { min-height: 0; overflow: auto; padding: 12px; border-right: 1px solid var(--color-border); color: var(--color-foreground); }
.folder-navigation { display: grid; gap: 4px; }
.folder-link { display: flex; align-items: center; gap: 7px; flex: 1; min-width: 0; min-height: 36px; padding: 7px 9px; border: 0; border-radius: 6px; background: transparent; color: var(--color-secondary-text); font: inherit; font-size: 13px; text-align: left; cursor: pointer; }
.folder-link span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.folder-link svg { flex: none; }
.folder-link small { margin-left: auto; }
.folder-link.active { background: var(--sui-bg-active); color: var(--sui-primary); }
.folder-link:hover, .folder-toggle:hover { background: var(--sui-bg-hover); }
.folder-row { display: flex; align-items: center; }
.folder-toggle, .folder-toggle-spacer { flex: none; width: 22px; }
.folder-toggle { display: grid; place-items: center; height: 36px; padding: 0; border: 0; background: transparent; color: inherit; cursor: pointer; }
.folder-toggle .expanded { transform: rotate(90deg); }
.folder-link:focus-visible, .folder-toggle:focus-visible, select:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: 1px; }
.folder-actions { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 16px; }
.folder-status { font-size: 12px; color: var(--color-secondary-text); overflow-wrap: anywhere; }
.folder-form, .folder-form label { display: grid; gap: 8px; }
.folder-form { gap: 16px; }
.folder-form p { margin: 0; color: var(--color-secondary-text); font-size: 12px; }
.folder-mobile-select { display: grid; gap: 5px; font-size: 12px; }
.folder-mobile-select select { min-width: 0; width: 100%; min-height: 44px; border: 1px solid var(--color-border); border-radius: 6px; padding: 8px; background: var(--color-background); color: var(--color-foreground); color-scheme: dark; }
@media (max-width: 768px) {
  .favorite-folders { padding: 10px 16px; border-right: 0; border-bottom: 1px solid var(--color-border); }
  .folder-actions { margin-top: 8px; }
  .folder-actions :deep(.s-button) { min-height: 44px; }
}
</style>
