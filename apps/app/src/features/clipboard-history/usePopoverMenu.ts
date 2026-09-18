import { nextTick, onBeforeUnmount, onMounted, ref, type Ref } from "vue";

/**
 * 下拉菜单骨架（TimeFilterControl / DeviceFilterControl 共用）：trigger +
 * Teleport 菜单的开合、定位、键盘导航、外点关闭、视口变化跟随。菜单宽度
 * 与打开后的初始焦点由各控件传入。
 */
export function usePopoverMenu(options: {
  trigger: Ref<HTMLElement | undefined>;
  menu: Ref<HTMLElement | undefined>;
  /** 菜单宽度（px）：定位时左右 clamp 进视口。 */
  width: number;
  /** 打开后要聚焦的首个控件的 selector。 */
  focusSelector: string;
}) {
  const menuOpen = ref(false);
  const menuStyle = ref<Record<string, string>>({});

  function positionMenu(): void {
    const rect = options.trigger.value?.getBoundingClientRect();
    if (!rect) return;
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - options.width - 8));
    menuStyle.value = { left: `${left}px`, top: `${rect.bottom + 6}px`, width: `${options.width}px` };
  }

  async function openMenu(): Promise<void> {
    await nextTick();
    positionMenu();
    options.menu.value?.querySelector<HTMLButtonElement>(options.focusSelector)?.focus();
  }

  async function toggleMenu(): Promise<void> {
    menuOpen.value = !menuOpen.value;
    if (menuOpen.value) await openMenu();
  }

  async function openMenuFromKeyboard(): Promise<void> {
    if (menuOpen.value) return;
    menuOpen.value = true;
    await openMenu();
  }

  function handleMenuKeydown(event: KeyboardEvent): void {
    event.stopPropagation();
    const buttons = Array.from(options.menu.value?.querySelectorAll<HTMLButtonElement>("button") ?? []);
    const currentIndex = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      menuOpen.value = false;
      options.trigger.value?.focus();
      return;
    }
    if (event.key === "Tab") {
      menuOpen.value = false;
      return;
    }
    const targetIndex = event.key === "ArrowDown"
      ? Math.min(buttons.length - 1, currentIndex + 1)
      : event.key === "ArrowUp"
        ? Math.max(0, currentIndex - 1)
        : event.key === "Home"
          ? 0
          : event.key === "End"
            ? buttons.length - 1
            : -1;
    if (targetIndex < 0) return;
    event.preventDefault();
    buttons[targetIndex]?.focus();
  }

  function handlePointerDown(event: PointerEvent): void {
    if (!menuOpen.value) return;
    const target = event.target as Node;
    if (options.trigger.value?.contains(target) || options.menu.value?.contains(target)) return;
    menuOpen.value = false;
  }

  function handleViewportChange(): void {
    if (menuOpen.value) positionMenu();
  }

  onMounted(() => {
    document.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("resize", handleViewportChange);
    window.addEventListener("scroll", handleViewportChange, true);
  });

  onBeforeUnmount(() => {
    document.removeEventListener("pointerdown", handlePointerDown);
    window.removeEventListener("resize", handleViewportChange);
    window.removeEventListener("scroll", handleViewportChange, true);
  });

  return { menuOpen, menuStyle, toggleMenu, openMenuFromKeyboard, handleMenuKeydown };
}
