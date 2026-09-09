/** Pure date/size formatting helpers shared by every window. */

/** Labels for the preset time filters; TimeFilterControl renders the same strings. */
export const TIME_FILTER_LABELS: Record<string, string> = {
  today: "今天",
  "7-days": "近 7 天",
  "30-days": "近 30 天",
};

export function formatAge(createdAt: string, now: number): string {
  const elapsed = Math.max(0, now - new Date(createdAt).getTime());
  const seconds = Math.floor(elapsed / 1_000);
  if (seconds < 10) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric" }).format(new Date(createdAt));
}

export function formatExactDateTime(createdAt: string): string {
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(createdAt));
}

export function parseLocalDate(value: string, endOfDay = false): Date | undefined {
  const [year, month, day] = value.split("-").map(Number);
  if (!year || !month || !day) return undefined;
  return new Date(year, month - 1, day, endOfDay ? 23 : 0, endOfDay ? 59 : 0, endOfDay ? 59 : 0, endOfDay ? 999 : 0);
}

/** Shared custom-date-range validation; returns the error message, or "" when valid. */
export function validateDateRange(start: string, end: string): string {
  if (!start || !end) return "请选择完整的开始和结束日期";
  if (start > end) return "开始日期不能晚于结束日期";
  return "";
}

export function formatFileSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}
