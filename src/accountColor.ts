/** 新建邮箱的推荐色板；顺序稳定，已有颜色越少越靠前。 */
export const ACCOUNT_COLOR_PALETTE = [
  "#3b82f6",
  "#10b981",
  "#f59e0b",
  "#ef4444",
  "#8b5cf6",
  "#ec4899",
  "#06b6d4",
  "#84cc16",
  "#f97316",
  "#14b8a6",
] as const;

function normalizeColor(color: string): string {
  return color.trim().toLowerCase();
}

/**
 * 优先推荐没用过的颜色；全部用过时，挑当前出现次数最少的那一个。
 * 用户填写的非色板颜色仍计入用量，不影响色板本身的顺序。
 */
export function recommendAccountColor(usedColors: readonly string[]): string {
  const counts = new Map<string, number>();
  for (const color of usedColors) {
    const normalized = normalizeColor(color);
    if (normalized === "") continue;
    counts.set(normalized, (counts.get(normalized) ?? 0) + 1);
  }

  for (const color of ACCOUNT_COLOR_PALETTE) {
    if (!counts.has(color)) return color;
  }

  let leastUsed: string = ACCOUNT_COLOR_PALETTE[0];
  let leastCount = counts.get(leastUsed) ?? 0;
  for (const color of ACCOUNT_COLOR_PALETTE.slice(1)) {
    const count = counts.get(color) ?? 0;
    if (count < leastCount) {
      leastUsed = color;
      leastCount = count;
    }
  }
  return leastUsed;
}
