//! 写信附件的纯函数：路径转附件、去重合并、拖拽命中判断。
//!
//! 只处理本地文件路径，不读文件内容、不联网。
//! 「选择文件」和「把文件拖进来」两条路最后都落到同一份 path + filename 列表。

import type { ComposeAttachment } from "./api";

/** 从路径里取文件名；取不到就用原串。 */
export function basename(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, "");
  if (trimmed === "") return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}

/**
 * 把一批本地路径转成附件项。
 * 去掉空串与首尾空格，按路径去重，顺序保持传入顺序。
 */
export function pathsToAttachments(paths: readonly string[]): ComposeAttachment[] {
  const seen = new Set<string>();
  const out: ComposeAttachment[] = [];
  for (const raw of paths) {
    const path = typeof raw === "string" ? raw.trim() : "";
    if (path === "" || seen.has(path)) continue;
    seen.add(path);
    out.push({ path, filename: basename(path) });
  }
  return out;
}

/** 合并附件列表：路径已在列表里的不再重复加，先有的项优先。 */
export function mergeAttachments(
  current: readonly ComposeAttachment[],
  incoming: readonly ComposeAttachment[],
): ComposeAttachment[] {
  const seen = new Set(current.map((item) => item.path));
  const out = current.slice();
  for (const item of incoming) {
    if (item.path === "" || seen.has(item.path)) continue;
    seen.add(item.path);
    out.push(item);
  }
  return out;
}

/** 拖拽坐标是物理像素，换算成网页里用的 CSS 像素。 */
export function toCssPoint(
  point: { x: number; y: number },
  devicePixelRatio: number,
): { x: number; y: number } {
  const ratio =
    Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  return { x: point.x / ratio, y: point.y / ratio };
}

/** 一个点在不在矩形里；矩形用 getBoundingClientRect 的结果。 */
export function pointInsideRect(
  point: { x: number; y: number },
  rect: { left: number; top: number; right: number; bottom: number },
): boolean {
  return (
    point.x >= rect.left &&
    point.x <= rect.right &&
    point.y >= rect.top &&
    point.y <= rect.bottom
  );
}