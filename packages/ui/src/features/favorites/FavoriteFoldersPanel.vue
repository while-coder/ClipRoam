<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onDeactivated, ref, useId, watch } from "vue";
import SCascadeMenu from "@qingfeng346/ui-kit/components/SCascadeMenu.vue";
import { ChevronRight, Folder, FolderOpen, Info, Menu, MoreHorizontal, Plus, Star } from "lucide-vue-next";
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
const currentPath = computed(() => props.modelValue === "unfiled" ? "未分类" : current.value?.path ?? "全部收藏");
const drawerOpen = ref(false);
const drawerId = useId();
const parentFolderLabelId = useId();
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
const moveError = ref("");
type FolderRow = typeof rows.value[number];
const tree = ref<HTMLElement>();
const focusedId = ref("");
watch(visibleRows, (visible) => {
  if (focusedId.value && focusedId.value !== "unfiled" && !visible.some((folder) => folder.id === focusedId.value)) focusedId.value = "";
});
const draggingId = ref<string>();
const dropTarget = ref<string>();
let pointerDrag: { id: number; x: number; y: number; folder: FolderRow; element: HTMLElement } | undefined;
let suppressClick = false;
const menu = ref<{ x: number; y: number; folder?: FolderRow }>();
const desktopMenu = ref<InstanceType<typeof SCascadeMenu>>();
function closeMenuOutside(event: PointerEvent): void {
  if (!desktopMenu.value?.contains(event.target as Node)) menu.value = undefined;
}
watch(() => Boolean(menu.value) && !mobile.value, (shown) => {
  document.removeEventListener("pointerdown", closeMenuOutside, true);
  if (shown) document.addEventListener("pointerdown", closeMenuOutside, true);
});
onBeforeUnmount(() => document.removeEventListener("pointerdown", closeMenuOutside, true));
const menuOptions = computed<Array<{ key: string; label: string; disabled?: boolean; error?: boolean }>>(() => [
  { key: "add", label: "新建一级目录" },
  ...(menu.value?.folder ? [
    { key: "child", label: "新建子目录", disabled: menu.value.folder.depth >= MAX_FAVORITE_FOLDER_DEPTH },
    { key: "rename", label: "重命名…" },
    { key: "move", label: "移动到…" },
    { key: "delete", label: "删除目录…", error: true },
  ] : []),
]);
onDeactivated(() => {
  drawerOpen.value = false;
  menu.value = undefined;
  endDrag();
  if (!busy.value) { editorOpen.value = false; deleteOpen.value = false; }
});
watch(mobile, () => { drawerOpen.value = false; menu.value = undefined; endDrag(); });
const allowedParents = computed(() => {
  const editing = rows.value.find((folder) => folder.id === editingId.value);
  return rows.value.filter((folder) => editing ? canParent(editing, folder.id) : folder.depth < MAX_FAVORITE_FOLDER_DEPTH).map((folder) => folder.id);
});

function openEditor(edit: boolean, child = false, target = current.value): void {
  if (busy.value) return;
  editingId.value = edit ? target?.id : undefined;
  name.value = edit ? target?.name ?? "" : "";
  parentId.value = edit ? target?.parentId ?? null : child ? target?.id ?? null : null;
  formError.value = "";
  editorOpen.value = true;
}
function requestDelete(folder: FolderRow): void {
  if (busy.value) return;
  formError.value = "";
  deleteTarget.value = folder;
  deleteOpen.value = true;
}
function openMenu(event: MouseEvent, folder?: FolderRow): void {
  if (busy.value || props.loading || props.error) return;
  (event.currentTarget as HTMLElement).focus();
  menu.value = { x: event.clientX, y: event.clientY, folder };
}
function openPanelMenu(event: MouseEvent): void {
  if (mobile.value) return;
  event.preventDefault();
  openMenu(event);
}
function openDrawer(): void {
  for (const folder of rows.value) {
    if (folder.descendantIds.includes(props.modelValue)) collapsed.value.delete(folder.id);
  }
  drawerOpen.value = true;
}
function navigateMobile(id: string): void {
  emit("update:modelValue", id);
  drawerOpen.value = false;
  menu.value = undefined;
}
function menuAction(key: string): void {
  const folder = menu.value?.folder;
  menu.value = undefined;
  if (key === "add") openEditor(false, false);
  else if (folder && key === "child") openEditor(false, true, folder);
  else if (folder && (key === "rename" || key === "move")) openEditor(true, false, folder);
  else if (folder && key === "delete") requestDelete(folder);
}
function canParent(source: FolderRow, targetId: string | null): boolean {
  if (targetId && source.descendantIds.includes(targetId)) return false;
  const target = rows.value.find((folder) => folder.id === targetId);
  const height = Math.max(...rows.value.filter((folder) => source.descendantIds.includes(folder.id)).map((folder) => folder.depth - source.depth));
  return (target?.depth ?? 0) + 1 + height <= MAX_FAVORITE_FOLDER_DEPTH;
}
// Pointer dragging also works in the Tauri WebView while native file drop stays enabled.
function startDrag(event: PointerEvent, folder: FolderRow): void {
  if (mobile.value || event.button !== 0 || event.pointerType === "touch" || busy.value || props.loading || props.error || (event.target as HTMLElement).closest("button")) return;
  suppressClick = false;
  pointerDrag = { id: event.pointerId, x: event.clientX, y: event.clientY, folder, element: event.currentTarget as HTMLElement };
  pointerDrag.element.setPointerCapture(event.pointerId);
}
function endDrag(): void {
  const pending = pointerDrag;
  pointerDrag = undefined;
  draggingId.value = undefined;
  dropTarget.value = undefined;
  if (pending?.element.hasPointerCapture(pending.id)) pending.element.releasePointerCapture(pending.id);
}
function dragMove(event: PointerEvent): void {
  if (!pointerDrag || pointerDrag.id !== event.pointerId) return;
  if (!draggingId.value && Math.hypot(event.clientX - pointerDrag.x, event.clientY - pointerDrag.y) < 6) return;
  event.preventDefault();
  menu.value = undefined;
  draggingId.value = pointerDrag.folder.id;
  suppressClick = true;
  const target = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-drop-folder]");
  const targetId = target?.dataset.dropFolder;
  dropTarget.value = target && tree.value?.contains(target) && targetId !== undefined && pointerDrag.folder.parentId !== (targetId || null) && canParent(pointerDrag.folder, targetId || null) ? targetId : undefined;
}
function finishDrag(event: PointerEvent): void {
  if (!pointerDrag || pointerDrag.id !== event.pointerId) return;
  const source = pointerDrag.folder;
  const targetId = dropTarget.value;
  endDrag();
  if (targetId !== undefined) void moveFolder(source, targetId || null);
}
function selectFolder(event: MouseEvent, folder: FolderRow): void {
  if (suppressClick) { suppressClick = false; event.preventDefault(); return; }
  focusItem(folder.id);
  emit("update:modelValue", folder.id);
}
async function moveFolder(source: FolderRow, targetId: string | null): Promise<void> {
  if (busy.value || source.parentId === targetId || !canParent(source, targetId)) return;
  const session = getAccountSession();
  if (!session) return;
  busy.value = true;
  moveError.value = "";
  try {
    await session.history.saveFavoriteFolder({ name: source.name, parentId: targetId }, source.id);
    if (session.signal.aborted) return;
    if (targetId) collapsed.value.delete(targetId);
    emit("refresh");
  } catch (error) { if (!session.signal.aborted) moveError.value = errorMessage(error); }
  finally { busy.value = false; }
}
function focusItem(id: string): void {
  focusedId.value = id;
  void nextTick(() => Array.from(tree.value?.querySelectorAll<HTMLElement>("[role=treeitem]") ?? []).find((item) => item.dataset.folderId === id)?.focus());
}
function treeKey(event: KeyboardEvent, id: string, folder?: FolderRow): void {
  const ids = ["", "unfiled", ...visibleRows.value.map((row) => row.id)];
  const index = ids.indexOf(id);
  if (event.key === "ArrowDown") focusItem(ids[Math.min(index + 1, ids.length - 1)]!);
  else if (event.key === "ArrowUp") focusItem(ids[Math.max(index - 1, 0)]!);
  else if (event.key === "Home") focusItem("");
  else if (event.key === "End") focusItem(ids[ids.length - 1]!);
  else if (event.key === "ArrowRight" && folder) {
    if (collapsed.value.has(id)) collapsed.value.delete(id);
    else { const child = visibleRows.value.find((row) => row.parentId === id); if (child) focusItem(child.id); }
  } else if (event.key === "ArrowLeft" && folder) {
    if (folder.descendantIds.length > 1 && !collapsed.value.has(id)) collapsed.value.add(id);
    else if (folder.parentId) focusItem(folder.parentId);
  } else if (event.key === "Enter" || event.key === " ") emit("update:modelValue", id);
  else if (event.key === "F2" && folder) openEditor(true, false, folder);
  else if (event.key === "Delete" && folder) requestDelete(folder);
  else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    if (!busy.value && !props.loading && !props.error) menu.value = { x: rect.left, y: rect.bottom, folder };
  } else return;
  event.preventDefault();
  event.stopPropagation();
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
    if (deleteTarget.value.descendantIds.includes(props.modelValue)) emit("update:modelValue", "unfiled");
    deleteOpen.value = false;
    emit("refresh");
  } catch (error) { if (!session.signal.aborted) formError.value = errorMessage(error); }
  finally { busy.value = false; }
}
</script>

<template>
  <aside class="favorite-folders" aria-label="收藏目录" @contextmenu="openPanelMenu">
    <button v-if="mobile" class="folder-mobile-trigger" :aria-label="`打开收藏目录，当前：${currentPath}`" aria-haspopup="dialog" :aria-expanded="drawerOpen" :aria-controls="drawerOpen ? drawerId : undefined" @click="openDrawer">
      <Menu :size="20" aria-hidden="true" /><span class="folder-trigger-label">目录</span><span class="folder-trigger-path" :title="currentPath">{{ currentPath }}</span><ChevronRight :size="16" aria-hidden="true" />
    </button>
    <template v-else>
      <div class="folder-heading"><strong>收藏目录</strong></div>
      <nav ref="tree" class="folder-navigation" role="tree" data-drop-folder="" aria-label="收藏目录导航" :aria-busy="busy || loading">
        <button class="folder-link" :class="{ active: !modelValue }" role="treeitem" data-folder-id="" :tabindex="focusedId === '' ? 0 : -1" :aria-selected="!modelValue" :aria-level="1" @focus="focusedId = ''" @keydown="treeKey($event, '')" @contextmenu.prevent.stop="openMenu($event)" @click="emit('update:modelValue', '')"><Star :size="16" aria-hidden="true" />全部收藏</button>
        <button class="folder-link" :class="{ active: modelValue === 'unfiled' }" role="treeitem" data-folder-id="unfiled" :tabindex="focusedId === 'unfiled' ? 0 : -1" :aria-selected="modelValue === 'unfiled'" :aria-level="1" @focus="focusedId = 'unfiled'" @keydown="treeKey($event, 'unfiled')" @contextmenu.prevent.stop="openMenu($event)" @click="emit('update:modelValue', 'unfiled')"><Folder :size="16" aria-hidden="true" />未分类</button>
        <div v-for="folder in visibleRows" :key="folder.id" class="folder-row" :class="{ active: modelValue === folder.id, dragging: draggingId === folder.id, 'drop-target': dropTarget === folder.id }" :style="{ paddingLeft: `${(folder.depth - 1) * 16}px` }"
          role="treeitem" :data-folder-id="folder.id" :data-drop-folder="folder.id" :tabindex="focusedId === folder.id ? 0 : -1" :aria-selected="modelValue === folder.id" :aria-level="folder.depth" :aria-expanded="folder.descendantIds.length > 1 ? !collapsed.has(folder.id) : undefined" :aria-label="`${folder.name}，${folder.count} 条收藏`"
          :title="folder.path" @focus="focusedId = folder.id" @keydown.self="treeKey($event, folder.id, folder)" @click="selectFolder($event, folder)"
          @contextmenu.prevent.stop="openMenu($event, folder)" @pointerdown="startDrag($event, folder)" @pointermove="dragMove" @pointerup="finishDrag" @pointercancel="endDrag" @lostpointercapture="endDrag" @dragstart.prevent>
          <button v-if="folder.descendantIds.length > 1" class="folder-toggle" tabindex="-1" :aria-label="`${collapsed.has(folder.id) ? '展开' : '收起'}${folder.name}`"
            @click.stop="focusItem(folder.id); collapsed.has(folder.id) ? collapsed.delete(folder.id) : collapsed.add(folder.id)"><ChevronRight :size="14" :class="{ expanded: !collapsed.has(folder.id) }" /></button>
          <span v-else class="folder-toggle-spacer" />
          <span class="folder-link"><component :is="folder.descendantIds.length > 1 && !collapsed.has(folder.id) ? FolderOpen : Folder" :size="15" aria-hidden="true" /><span>{{ folder.name }}</span><small>{{ folder.count }}</small></span>
          <button class="folder-icon-button folder-more" tabindex="-1" :disabled="busy || loading || !!error" :aria-label="`${folder.name}的操作`" title="目录操作" @click.stop="openMenu($event, folder)"><MoreHorizontal :size="15" /></button>
        </div>
        <div class="folder-root-drop" data-drop-folder="" @contextmenu.prevent.stop="openMenu($event)"><Info :size="13" aria-hidden="true" /><span>右键管理目录 · 拖动移动</span></div>
      </nav>
      <SCascadeMenu v-if="menu" ref="desktopMenu" :show="true" :root-style="{ left: `${menu.x}px`, top: `${menu.y}px` }" :options="menuOptions" label="收藏目录操作" @close="menu = undefined" @select="option => menuAction(String(option.key))" />
      <p v-if="moveError" class="folder-status" role="alert">{{ moveError }}</p>
    </template>
    <template v-if="!mobile">
      <div v-if="error" class="folder-status" role="alert">{{ error }} <SButton size="small" @click="emit('refresh')">重试</SButton></div>
    </template>
  </aside>

  <SModal v-if="mobile" :id="drawerId" v-model:show="drawerOpen" class="favorite-folder-drawer" title="收藏目录">
    <nav class="mobile-folder-navigation" aria-label="收藏目录导航">
      <button class="folder-link" :class="{ active: !modelValue }" :aria-current="!modelValue ? 'page' : undefined" @click="navigateMobile('')"><Star :size="18" aria-hidden="true" />全部收藏</button>
      <button class="folder-link" :class="{ active: modelValue === 'unfiled' }" :aria-current="modelValue === 'unfiled' ? 'page' : undefined" @click="navigateMobile('unfiled')"><Folder :size="18" aria-hidden="true" />未分类</button>
      <div v-for="folder in visibleRows" :key="folder.id" class="mobile-folder-row" :class="{ active: modelValue === folder.id }" :style="{ paddingLeft: `${(folder.depth - 1) * 16}px` }">
        <button v-if="folder.descendantIds.length > 1" class="folder-toggle" :aria-label="`${collapsed.has(folder.id) ? '展开' : '收起'}${folder.name}`" :aria-expanded="!collapsed.has(folder.id)" @click="collapsed.has(folder.id) ? collapsed.delete(folder.id) : collapsed.add(folder.id)"><ChevronRight :size="16" :class="{ expanded: !collapsed.has(folder.id) }" /></button>
        <span v-else class="folder-toggle-spacer" />
        <button class="folder-link" :aria-current="modelValue === folder.id ? 'page' : undefined" :title="folder.path" @click="navigateMobile(folder.id)"><Folder :size="18" aria-hidden="true" /><span>{{ folder.name }}</span><small>{{ folder.count }}</small></button>
        <button class="folder-icon-button" :disabled="busy || loading || !!error" :aria-label="`${folder.name}的操作`" @click="openMenu($event, folder)"><MoreHorizontal :size="18" /></button>
      </div>
    </nav>
    <p v-if="loading" class="folder-status" role="status">正在读取目录…</p>
    <div v-if="error" class="folder-status" role="alert">{{ error }} <SButton @click="emit('refresh')">重试</SButton></div>
    <p v-if="busy" class="folder-status" role="status">正在保存目录…</p>
    <template #footer><SButton block :disabled="busy || loading || !!error" @click="openEditor(false)"><Plus :size="18" aria-hidden="true" />新建目录</SButton></template>
  </SModal>
  <SModal v-if="mobile && menu" :show="true" :title="menu.folder?.name ?? '目录操作'" class="favorite-folder-actions" nested @update:show="menu = undefined">
    <div class="mobile-folder-actions"><SButton v-for="option in menuOptions" :key="option.key" block :type="option.error ? 'error' : 'default'" :disabled="option.disabled" @click="menuAction(option.key)">{{ option.label }}</SButton></div>
  </SModal>

  <SModal v-if="editorOpen" :show="true" :nested="mobile && drawerOpen" :title="editingId ? '编辑目录' : '新建目录'" width="440px" :closable="!busy" :mask-closable="!busy" :close-on-esc="!busy" @update:show="!busy && (editorOpen = false)">
    <form class="folder-form" @submit.prevent="save">
      <label>目录名称<SInput v-model:value="name" autofocus maxlength="80" :disabled="busy" /></label>
      <div class="folder-select-field"><span :id="parentFolderLabelId">父目录</span><FavoriteFolderSelect v-model="parentId" :aria-labelledby="parentFolderLabelId" :folders="folders" :allowed-ids="allowedParents" null-label="无（一级目录）" :disabled="busy" /></div>
      <p>最多支持 {{ MAX_FAVORITE_FOLDER_DEPTH }} 级目录。修改父目录即可移动目录。</p>
      <p v-if="formError" role="alert">{{ formError }}</p>
      <button type="submit" hidden />
    </form>
    <template #footer><SButton :disabled="busy" @click="editorOpen = false">取消</SButton><SButton type="primary" :loading="busy" :disabled="!name.trim()" @click="save">保存</SButton></template>
  </SModal>
  <SModal v-if="deleteOpen" :show="true" :nested="mobile && drawerOpen" title="删除目录" width="440px" :closable="!busy" :mask-closable="!busy" :close-on-esc="!busy" @update:show="!busy && (deleteOpen = false)">
    <p>删除「{{ deleteTarget?.name }}」及其 {{ (deleteTarget?.descendantIds.length ?? 1) - 1 }} 个下级目录？其中 {{ deleteTarget?.count ?? 0 }} 条收藏会移到「未分类」，内容仍保持收藏。</p>
    <p v-if="formError" role="alert">{{ formError }}</p>
    <template #footer><SButton :disabled="busy" @click="deleteOpen = false">取消</SButton><SButton type="error" :loading="busy" @click="remove">删除目录，保留内容</SButton></template>
  </SModal>
</template>

<style scoped>
.favorite-folders { display: flex; flex-direction: column; min-height: 0; overflow: auto; padding: 12px; border-right: 1px solid var(--color-border); color: var(--color-foreground); }
.folder-heading { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; font-size: 12px; color: var(--color-secondary-text); }
.folder-navigation { display: flex; flex-direction: column; flex: 1 0 auto; gap: 4px; }
.folder-navigation > * { flex: none; }
.folder-icon-button { display: grid; place-items: center; flex: none; width: 28px; height: 28px; padding: 0; border: 0; border-radius: 5px; background: transparent; color: var(--color-secondary-text); cursor: pointer; }
.folder-icon-button:hover { background: var(--sui-bg-hover); }
.folder-icon-button:disabled { opacity: .5; cursor: default; }
.folder-more { opacity: 0; }
.folder-row:hover .folder-more, .folder-row:focus-within .folder-more { opacity: 1; }
.folder-row.active { background: var(--sui-bg-active); color: var(--sui-primary); }
.folder-row.active .folder-link { color: inherit; }
.folder-row:hover { background: var(--sui-bg-hover); }
.folder-row { border-radius: 6px; cursor: pointer; user-select: none; touch-action: pan-y; }
.folder-row.dragging { opacity: .45; }
.folder-root-drop { display: flex; align-items: center; justify-content: center; gap: 6px; padding: 10px 4px; margin-top: auto; font-size: 11px; line-height: 1.5; color: var(--color-secondary-text); }
.folder-root-drop > svg { flex: none; }
.folder-navigation .drop-target { outline: 2px solid var(--sui-primary); outline-offset: -2px; background: var(--sui-bg-active); }
.folder-row:focus-visible, .folder-icon-button:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: 1px; }
.folder-link { display: flex; align-items: center; gap: 7px; flex: none; min-width: 0; min-height: 36px; box-sizing: border-box; padding: 7px 9px; border: 0; border-radius: 6px; background: transparent; color: var(--color-secondary-text); font: inherit; font-size: 13px; text-align: left; cursor: pointer; }
.folder-row > .folder-link, .mobile-folder-row > .folder-link { flex: 1; }
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
.folder-status { font-size: 12px; color: var(--color-secondary-text); overflow-wrap: anywhere; }
.folder-form, .folder-form label, .folder-select-field { display: grid; gap: 8px; }
.folder-form { gap: 16px; }
.folder-form p { margin: 0; color: var(--color-secondary-text); font-size: 12px; }
.folder-mobile-trigger { display: flex; align-items: center; gap: 10px; width: 100%; min-height: 44px; padding: 0; border: 0; background: transparent; color: var(--color-foreground); font: inherit; cursor: pointer; text-align: left; }
.folder-mobile-trigger svg, .folder-trigger-label { flex: none; }
.folder-trigger-label { font-size: 14px; }
.folder-trigger-path { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; color: var(--color-secondary-text); }
.folder-mobile-trigger:focus-visible { outline: 2px solid var(--sui-primary); outline-offset: 2px; }
.mobile-folder-navigation { display: grid; gap: 4px; }
.mobile-folder-row { display: flex; align-items: center; min-width: 0; border-radius: 6px; }
.mobile-folder-row.active { color: var(--sui-primary); background: var(--sui-bg-active); }
.mobile-folder-row.active .folder-link { color: inherit; }
.mobile-folder-navigation .folder-link { min-height: 48px; font-size: 15px; }
.mobile-folder-navigation .folder-icon-button, .mobile-folder-navigation .folder-toggle { width: 44px; height: 48px; flex: none; }
.mobile-folder-navigation .folder-toggle-spacer { width: 44px; }
.mobile-folder-actions { display: grid; gap: 8px; }
.mobile-folder-actions :deep(.s-button), .mobile-folder-navigation .s-button { min-height: 44px; }
.folder-mobile-trigger:active, .mobile-folder-navigation button:active { background: var(--sui-bg-hover); }
:global(.favorite-folder-drawer.s-modal-overlay) { padding: 0; align-items: stretch; justify-content: flex-start; }
:global(.favorite-folder-drawer .s-modal-box) { width: min(340px, 86vw) !important; height: 100%; max-height: 100%; border: 0; border-right: 1px solid var(--color-border); border-radius: 0; padding-left: env(safe-area-inset-left, 0px); box-sizing: border-box; }
:global(.favorite-folder-drawer .s-modal-header) { flex: none; padding-top: max(12px, env(safe-area-inset-top, 0px)); }
:global(.favorite-folder-drawer .s-icon-close) { width: 44px; height: 44px; }
:global(.favorite-folder-drawer .s-modal-body) { flex: 1; padding: 12px; }
:global(.favorite-folder-drawer .s-modal-footer) { flex: none; padding-bottom: max(12px, env(safe-area-inset-bottom, 0px)); }
:global(.favorite-folder-drawer .s-modal-footer .s-button) { min-height: 44px; }
:global(.favorite-folder-drawer.s-modal-enter-active), :global(.favorite-folder-drawer.s-modal-leave-active) { transition: opacity 200ms ease; }
:global(.favorite-folder-drawer.s-modal-enter-active .s-modal-box), :global(.favorite-folder-drawer.s-modal-leave-active .s-modal-box) { transition: transform 200ms ease; }
:global(.favorite-folder-drawer.s-modal-enter-from .s-modal-box), :global(.favorite-folder-drawer.s-modal-leave-to .s-modal-box) { transform: translateX(-100%); }
@media (prefers-reduced-motion: reduce) {
  :global(.favorite-folder-drawer.s-modal-overlay), :global(.favorite-folder-drawer .s-modal-box) { transition: none; }
}
@media (max-width: 768px) {
  .favorite-folders { padding: 4px 16px; border-right: 0; border-bottom: 1px solid var(--color-border); }
}
</style>
