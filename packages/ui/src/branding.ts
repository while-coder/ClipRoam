import { computed, ref } from "vue";

const locale = ref(typeof navigator === "undefined" ? "en" : navigator.language);
export const isChinese = computed(() => /^zh(?:[-_]|$)/i.test(locale.value));
export const appName = computed(() => isChinese.value ? "随心贴" : "ClipRoam");
export const quickPasteTitle = computed(() => `${appName.value} - ${isChinese.value ? "快速粘贴" : "Quick Paste"}`);

/** Native clients use the OS locale; browser clients use the browser locale. */
export function configureAppLocale(value?: string): void {
  if (value?.trim()) locale.value = value.trim();
}
