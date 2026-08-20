<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { CalendarDays, Check, ChevronDown, ChevronLeft, ChevronRight, X } from "lucide-vue-next";

type TimeFilterValue = "all" | "today" | "7-days" | "30-days" | "custom";
type CalendarDay = { key: string; label: number; inMonth: boolean };

const props = defineProps<{
  modelValue: TimeFilterValue;
  startDate: string;
  endDate: string;
  error?: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: TimeFilterValue];
  "update:startDate": [value: string];
  "update:endDate": [value: string];
}>();

const options: Array<{ value: TimeFilterValue; label: string }> = [
  { value: "all", label: "不限" },
  { value: "today", label: "今天" },
  { value: "7-days", label: "近 7 天" },
  { value: "30-days", label: "近 30 天" },
  { value: "custom", label: "自定义区间" },
];
const weekdays = ["一", "二", "三", "四", "五", "六", "日"];

const trigger = ref<HTMLButtonElement>();
const menu = ref<HTMLElement>();
const calendarDialog = ref<HTMLElement>();
const menuOpen = ref(false);
const calendarOpen = ref(false);
const menuStyle = ref<Record<string, string>>({});
const displayMonth = ref(startOfMonth(new Date()));
const draftStartDate = ref("");
const draftEndDate = ref("");
const selectingBoundary = ref<"start" | "end">("start");

const currentLabel = computed(() => (
  options.find((option) => option.value === props.modelValue)?.label ?? "不限"
));
const displayMonthLabel = computed(() => `${displayMonth.value.getFullYear()} 年 ${displayMonth.value.getMonth() + 1} 月`);
const draftError = computed(() => {
  if (!draftStartDate.value || !draftEndDate.value) return "请选择完整的开始和结束日期";
  if (draftStartDate.value > draftEndDate.value) return "开始日期不能晚于结束日期";
  return "";
});
const calendarDays = computed<CalendarDay[]>(() => {
  const year = displayMonth.value.getFullYear();
  const month = displayMonth.value.getMonth();
  const firstDay = new Date(year, month, 1);
  const mondayOffset = (firstDay.getDay() + 6) % 7;
  const gridStart = new Date(year, month, 1 - mondayOffset);
  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(gridStart);
    date.setDate(gridStart.getDate() + index);
    return {
      key: formatDate(date),
      label: date.getDate(),
      inMonth: date.getMonth() === month,
    };
  });
});

function startOfMonth(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), 1);
}

function formatDate(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function parseDate(value: string): Date | undefined {
  const [year, month, day] = value.split("-").map(Number);
  if (!year || !month || !day) return undefined;
  return new Date(year, month - 1, day);
}

function displayDate(value: string): string {
  return value ? value.replace(/-/g, "/") : "请选择";
}

function positionMenu(): void {
  const rect = trigger.value?.getBoundingClientRect();
  if (!rect) return;
  const width = 172;
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
  menuStyle.value = { left: `${left}px`, top: `${rect.bottom + 6}px`, width: `${width}px` };
}

async function toggleMenu(): Promise<void> {
  menuOpen.value = !menuOpen.value;
  if (!menuOpen.value) return;
  await nextTick();
  positionMenu();
  menu.value?.querySelector<HTMLButtonElement>("[aria-selected='true']")?.focus();
}

async function openMenuFromKeyboard(): Promise<void> {
  if (menuOpen.value) return;
  menuOpen.value = true;
  await nextTick();
  positionMenu();
  menu.value?.querySelector<HTMLButtonElement>("[aria-selected='true']")?.focus();
}

function handleMenuKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  const buttons = Array.from(menu.value?.querySelectorAll<HTMLButtonElement>("button") ?? []);
  const currentIndex = buttons.indexOf(document.activeElement as HTMLButtonElement);
  if (event.key === "Escape") {
    event.preventDefault();
    menuOpen.value = false;
    trigger.value?.focus();
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

function selectOption(value: TimeFilterValue): void {
  menuOpen.value = false;
  if (value === "custom") {
    openCalendar();
    return;
  }
  emit("update:modelValue", value);
  trigger.value?.focus();
}

async function openCalendar(): Promise<void> {
  const today = new Date();
  const defaultStart = new Date(today);
  defaultStart.setDate(defaultStart.getDate() - 6);
  draftStartDate.value = props.startDate || formatDate(defaultStart);
  draftEndDate.value = props.endDate || formatDate(today);
  selectingBoundary.value = "start";
  displayMonth.value = startOfMonth(parseDate(draftEndDate.value) ?? today);
  calendarOpen.value = true;
  await nextTick();
  calendarDialog.value?.focus();
}

function closeCalendar(): void {
  calendarOpen.value = false;
  nextTick(() => trigger.value?.focus());
}

function applyCalendar(): void {
  if (draftError.value) return;
  emit("update:startDate", draftStartDate.value);
  emit("update:endDate", draftEndDate.value);
  emit("update:modelValue", "custom");
  calendarOpen.value = false;
  nextTick(() => trigger.value?.focus());
}

function selectDay(value: string): void {
  if (selectingBoundary.value === "start") {
    draftStartDate.value = value;
    if (draftEndDate.value && value > draftEndDate.value) draftEndDate.value = "";
    selectingBoundary.value = "end";
    return;
  }
  if (!draftStartDate.value || value < draftStartDate.value) {
    draftStartDate.value = value;
    draftEndDate.value = "";
    selectingBoundary.value = "end";
    return;
  }
  draftEndDate.value = value;
  selectingBoundary.value = "start";
}

function changeMonth(offset: number): void {
  displayMonth.value = new Date(displayMonth.value.getFullYear(), displayMonth.value.getMonth() + offset, 1);
}

function dayClass(day: CalendarDay): Record<string, boolean> {
  return {
    muted: !day.inMonth,
    today: day.key === formatDate(new Date()),
    "range-start": day.key === draftStartDate.value,
    "range-end": day.key === draftEndDate.value,
    "in-range": Boolean(draftStartDate.value && draftEndDate.value && day.key > draftStartDate.value && day.key < draftEndDate.value),
  };
}

function handlePointerDown(event: PointerEvent): void {
  if (!menuOpen.value) return;
  const target = event.target as Node;
  if (trigger.value?.contains(target) || menu.value?.contains(target)) return;
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
</script>

<template>
  <div class="time-filter-control" :class="{ invalid: error }">
    <span class="time-filter-label">时间</span>
    <button
      ref="trigger"
      class="time-filter-trigger"
      type="button"
      aria-haspopup="listbox"
      :aria-expanded="menuOpen"
      :title="error || `时间筛选：${currentLabel}`"
      @click="toggleMenu"
      @keydown.stop
      @keydown.arrow-down.prevent="openMenuFromKeyboard"
      @keydown.arrow-up.prevent="openMenuFromKeyboard"
    >
      <span>{{ currentLabel }}</span>
      <ChevronDown :size="14" aria-hidden="true" />
    </button>
  </div>

  <Teleport to="body">
    <div
      v-if="menuOpen"
      ref="menu"
      class="time-filter-menu"
      role="listbox"
      aria-label="时间筛选"
      :style="menuStyle"
      @keydown="handleMenuKeydown"
    >
      <button
        v-for="option in options"
        :key="option.value"
        type="button"
        role="option"
        :aria-selected="modelValue === option.value"
        :class="{ active: modelValue === option.value }"
        @click="selectOption(option.value)"
      >
        <Check :size="14" :class="{ hidden: modelValue !== option.value }" aria-hidden="true" />
        <span>{{ option.label }}</span>
      </button>
    </div>

    <div v-if="calendarOpen" class="time-calendar-backdrop" @mousedown.self="closeCalendar">
      <section
        ref="calendarDialog"
        class="time-calendar-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="time-calendar-heading"
        tabindex="-1"
        @keydown.stop.escape.prevent="closeCalendar"
      >
        <header class="time-calendar-header">
          <div>
            <span>时间筛选</span>
            <h2 id="time-calendar-heading">选择日期区间</h2>
          </div>
          <button type="button" title="关闭日期选择" aria-label="关闭日期选择" @click="closeCalendar"><X :size="18" /></button>
        </header>

        <div class="time-calendar-boundaries">
          <button type="button" :class="{ active: selectingBoundary === 'start' }" @click="selectingBoundary = 'start'">
            <span>开始日期</span><strong>{{ displayDate(draftStartDate) }}</strong>
          </button>
          <span aria-hidden="true">至</span>
          <button type="button" :class="{ active: selectingBoundary === 'end' }" @click="selectingBoundary = 'end'">
            <span>结束日期</span><strong>{{ displayDate(draftEndDate) }}</strong>
          </button>
        </div>

        <div class="time-calendar-month-header">
          <button type="button" title="上个月" aria-label="上个月" @click="changeMonth(-1)"><ChevronLeft :size="18" /></button>
          <strong>{{ displayMonthLabel }}</strong>
          <button type="button" title="下个月" aria-label="下个月" @click="changeMonth(1)"><ChevronRight :size="18" /></button>
        </div>
        <div class="time-calendar-weekdays" aria-hidden="true"><span v-for="weekday in weekdays" :key="weekday">{{ weekday }}</span></div>
        <div class="time-calendar-grid" role="grid" aria-label="日期">
          <button
            v-for="day in calendarDays"
            :key="day.key"
            type="button"
            role="gridcell"
            :class="dayClass(day)"
            :aria-label="day.key"
            :aria-selected="day.key === draftStartDate || day.key === draftEndDate"
            @click="selectDay(day.key)"
          >{{ day.label }}</button>
        </div>

        <p v-if="draftError" class="time-calendar-error" role="alert">{{ draftError }}</p>
        <footer class="time-calendar-actions">
          <button class="calendar-secondary" type="button" @click="closeCalendar">取消</button>
          <button class="calendar-primary" type="button" :disabled="Boolean(draftError)" @click="applyCalendar">
            <CalendarDays :size="16" aria-hidden="true" />应用区间
          </button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.time-filter-control { display: flex; flex: 0 0 auto; align-items: center; height: 27px; gap: 5px; margin-left: 5px; padding-left: 9px; border-left: 1px solid rgba(148, 163, 184, 0.16); white-space: nowrap; }
.time-filter-label { color: #64748b; font-size: 11px; }
.time-filter-trigger { display: flex; align-items: center; justify-content: space-between; min-width: 106px; height: 27px; gap: 8px; padding: 0 7px 0 9px; color: #cbd5e1; background: rgba(255, 255, 255, 0.04); border: 1px solid rgba(148, 163, 184, 0.18); border-radius: 6px; font-size: 11px; cursor: pointer; }
.time-filter-trigger:hover { color: #e2e8f0; background: rgba(255, 255, 255, 0.07); }
.time-filter-trigger:focus-visible { border-color: #60a5fa; outline: 0; box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.18); }
.invalid .time-filter-trigger { color: #fecaca; border-color: rgba(248, 113, 113, 0.58); }
.time-filter-menu { position: fixed; z-index: 60; display: grid; gap: 2px; padding: 5px; background: #111c31; border: 1px solid rgba(148, 163, 184, 0.24); border-radius: 8px; box-shadow: 0 14px 36px rgba(2, 6, 23, 0.62); }
.time-filter-menu button { display: grid; grid-template-columns: 18px 1fr; align-items: center; min-height: 34px; padding: 0 9px; color: #cbd5e1; text-align: left; background: transparent; border: 0; border-radius: 5px; font-size: 12px; cursor: pointer; }
.time-filter-menu button:hover, .time-filter-menu button:focus-visible { color: #f8fafc; background: rgba(96, 165, 250, 0.13); outline: 0; }
.time-filter-menu button.active { color: #dbeafe; background: #1e3a5f; }
.hidden { visibility: hidden; }
.time-calendar-backdrop { position: fixed; z-index: 70; inset: 0; display: grid; place-items: center; padding: 20px; background: rgba(2, 6, 23, 0.7); }
.time-calendar-dialog { width: min(100%, 360px); padding: 16px; background: #111c31; border: 1px solid rgba(96, 165, 250, 0.28); border-radius: 12px; box-shadow: 0 22px 64px rgba(2, 6, 23, 0.7); outline: 0; }
.time-calendar-header { display: flex; align-items: center; justify-content: space-between; }
.time-calendar-header > div { display: grid; gap: 2px; }
.time-calendar-header span { color: #64748b; font-size: 10px; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; }
.time-calendar-header h2 { margin: 0; color: #f8fafc; font-size: 16px; }
.time-calendar-header button, .time-calendar-month-header button { display: grid; place-items: center; width: 34px; height: 34px; padding: 0; color: #94a3b8; background: transparent; border: 0; border-radius: 7px; cursor: pointer; }
.time-calendar-header button:hover, .time-calendar-month-header button:hover { color: #f8fafc; background: rgba(255, 255, 255, 0.07); }
.time-calendar-boundaries { display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: 8px; margin-top: 15px; }
.time-calendar-boundaries > span { color: #64748b; font-size: 11px; }
.time-calendar-boundaries button { display: grid; gap: 2px; min-width: 0; min-height: 48px; padding: 6px 9px; color: #94a3b8; text-align: left; background: #0f172a; border: 1px solid rgba(148, 163, 184, 0.18); border-radius: 7px; cursor: pointer; }
.time-calendar-boundaries button.active { border-color: #60a5fa; box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.14); }
.time-calendar-boundaries span { font-size: 10px; }
.time-calendar-boundaries strong { overflow: hidden; color: #e2e8f0; font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
.time-calendar-month-header { display: grid; grid-template-columns: 34px 1fr 34px; align-items: center; margin-top: 12px; }
.time-calendar-month-header strong { color: #e2e8f0; text-align: center; font-size: 13px; }
.time-calendar-weekdays, .time-calendar-grid { display: grid; grid-template-columns: repeat(7, 1fr); }
.time-calendar-weekdays { margin-top: 5px; color: #64748b; font-size: 10px; text-align: center; }
.time-calendar-weekdays span { padding: 5px 0; }
.time-calendar-grid { gap: 2px; }
.time-calendar-grid button { position: relative; height: 34px; padding: 0; color: #cbd5e1; background: transparent; border: 0; border-radius: 6px; font-size: 11px; cursor: pointer; }
.time-calendar-grid button:hover, .time-calendar-grid button:focus-visible { color: #f8fafc; background: rgba(96, 165, 250, 0.14); outline: 0; }
.time-calendar-grid button.muted { color: #475569; }
.time-calendar-grid button.today::after { position: absolute; bottom: 3px; left: 50%; width: 3px; height: 3px; content: ""; background: #60a5fa; border-radius: 50%; transform: translateX(-50%); }
.time-calendar-grid button.in-range { color: #dbeafe; background: rgba(37, 99, 235, 0.18); border-radius: 0; }
.time-calendar-grid button.range-start, .time-calendar-grid button.range-end { color: #eff6ff; background: #2563eb; }
.time-calendar-error { margin: 9px 0 0; color: #fca5a5; font-size: 11px; }
.time-calendar-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; padding-top: 12px; border-top: 1px solid rgba(148, 163, 184, 0.14); }
.time-calendar-actions button { display: flex; align-items: center; justify-content: center; min-width: 96px; min-height: 38px; gap: 6px; padding: 0 12px; border-radius: 7px; font-size: 12px; font-weight: 600; cursor: pointer; }
.calendar-secondary { color: #cbd5e1; background: transparent; border: 1px solid rgba(148, 163, 184, 0.22); }
.calendar-primary { color: #dbeafe; background: #1e3a5f; border: 1px solid rgba(96, 165, 250, 0.34); }
.calendar-primary:disabled { cursor: not-allowed; opacity: 0.5; }
@media (max-width: 640px) {
  .time-filter-control { height: 44px; margin-left: 0; padding-left: 12px; }
  .time-filter-label { font-size: 13px; }
  .time-filter-trigger { min-width: 126px; height: 44px; padding-inline: 12px 9px; font-size: 13px; }
  .time-filter-menu button { min-height: 44px; font-size: 14px; }
  .time-calendar-backdrop { padding: 12px; }
  .time-calendar-dialog { padding: 14px; }
  .time-calendar-grid button { height: 40px; font-size: 13px; }
  .time-calendar-actions button { min-height: 44px; }
}
</style>
