//! 页面缩放、字体大小与字体样式：三个只影响本机显示的外观偏好。
//!
//! - 页面缩放：走 WebView 的原生缩放，整个界面（含读信正文）一起放大缩小；
//! - 字体大小：改根节点的基准字号，界面文字跟着变，布局宽度不动；
//! - 字体样式：改根节点的字体栈（系统默认 / 雅黑 / 等线 / 宋体 / 黑体 / 楷体）。
//!
//! 三者都只存本地，不碰邮件数据，也不发任何网络请求。

import { useCallback, useEffect, useState } from "react";

import { api } from "./api";

/** 本地存储键：页面缩放比例。 */
export const UI_ZOOM_KEY = "ymail.ui-zoom.v1";
/** 本地存储键：界面基准字号（像素）。 */
export const UI_FONT_KEY = "ymail.ui-font.v1";
/** 本地存储键：字体样式。 */
export const UI_FONT_FAMILY_KEY = "ymail.ui-font-family.v1";

/** 页面缩放候选档位；范围 80%–150%。 */
export const UI_ZOOM_LEVELS = [0.8, 0.9, 1, 1.1, 1.25, 1.5] as const;
export const UI_ZOOM_MIN = UI_ZOOM_LEVELS[0];
export const UI_ZOOM_MAX = UI_ZOOM_LEVELS[UI_ZOOM_LEVELS.length - 1];
export const UI_ZOOM_DEFAULT = 1;

/** 界面基准字号候选（像素）：13 / 15 / 17 / 19。 */
export const UI_FONT_LEVELS = [13, 15, 17, 19] as const;
export const UI_FONT_MIN = UI_FONT_LEVELS[0];
export const UI_FONT_MAX = UI_FONT_LEVELS[UI_FONT_LEVELS.length - 1];
export const UI_FONT_DEFAULT = 15;

/** 可选字体样式；值是本地存储里的标识，别直接当字体名用。 */
export type UiFontFamily = "system" | "yahei" | "dengxian" | "simsun" | "simhei" | "kaiti";

export const UI_FONT_FAMILY_DEFAULT: UiFontFamily = "system";

/** 顺序即界面下拉顺序。 */
export const UI_FONT_FAMILIES: readonly UiFontFamily[] = [
  "system",
  "yahei",
  "dengxian",
  "simsun",
  "simhei",
  "kaiti",
];

/** 每种字体样式对应的 CSS 字体栈；系统默认跟 index.css 里的原来一致。 */
export const UI_FONT_FAMILY_STACKS: Record<UiFontFamily, string> = {
  system: '"Segoe UI", "Microsoft YaHei", system-ui, sans-serif',
  yahei: '"Microsoft YaHei", "Segoe UI", system-ui, sans-serif',
  dengxian: '"DengXian", "Microsoft YaHei", system-ui, sans-serif',
  simsun: '"SimSun", "宋体", serif',
  simhei: '"SimHei", "黑体", sans-serif',
  kaiti: '"KaiTi", "楷体", serif',
};

/** 字体样式在界面上的中文名。 */
export const UI_FONT_FAMILY_LABELS: Record<UiFontFamily, string> = {
  system: "系统默认",
  yahei: "微软雅黑",
  dengxian: "等线",
  simsun: "宋体",
  simhei: "黑体",
  kaiti: "楷体",
};

/** 同一窗口里改设置后通知其它组件刷新；浏览器自带的 storage 事件只在别的窗口触发。 */
const UI_SCALE_EVENT = "ymail:ui-scale-change";

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** 收进 80%–150%；非法值退回 100%。 */
export function normalizeUiZoom(value: number): number {
  if (!Number.isFinite(value)) return UI_ZOOM_DEFAULT;
  return clamp(value, UI_ZOOM_MIN, UI_ZOOM_MAX);
}

/** 收进 13–19 像素并取整；非法值退回标准字号。 */
export function normalizeUiFont(value: number): number {
  if (!Number.isFinite(value)) return UI_FONT_DEFAULT;
  return Math.round(clamp(value, UI_FONT_MIN, UI_FONT_MAX));
}

/** 只认白名单里的字体样式，别的都当没设置过。 */
export function isUiFontFamily(value: unknown): value is UiFontFamily {
  return typeof value === "string" && (UI_FONT_FAMILIES as readonly string[]).includes(value);
}

export function normalizeUiFontFamily(value: unknown): UiFontFamily {
  return isUiFontFamily(value) ? value : UI_FONT_FAMILY_DEFAULT;
}

/** 读本地存储里的缩放比例；读不到 / 存坏了就用 100%。 */
export function readStoredUiZoom(): number {
  try {
    const raw = window.localStorage.getItem(UI_ZOOM_KEY);
    if (raw === null) return UI_ZOOM_DEFAULT;
    return normalizeUiZoom(Number(raw));
  } catch {
    return UI_ZOOM_DEFAULT;
  }
}

/** 读本地存储里的基准字号；读不到 / 存坏了就用标准字号。 */
export function readStoredUiFont(): number {
  try {
    const raw = window.localStorage.getItem(UI_FONT_KEY);
    if (raw === null) return UI_FONT_DEFAULT;
    return normalizeUiFont(Number(raw));
  } catch {
    return UI_FONT_DEFAULT;
  }
}

/** 读本地存储里的字体样式；读不到 / 存坏了就用系统默认。 */
export function readStoredUiFontFamily(): UiFontFamily {
  try {
    return normalizeUiFontFamily(window.localStorage.getItem(UI_FONT_FAMILY_KEY));
  } catch {
    return UI_FONT_FAMILY_DEFAULT;
  }
}

/** 把基准字号写到根节点；读信正文由读信组件单独带上同一个值。 */
export function applyUiFont(sizePx: number) {
  document.documentElement.style.fontSize = `${normalizeUiFont(sizePx)}px`;
}

/** 把字体栈写到根节点；读信正文由读信组件单独带上同一个栈。 */
export function applyUiFontFamily(family: UiFontFamily) {
  document.documentElement.style.fontFamily =
    UI_FONT_FAMILY_STACKS[normalizeUiFontFamily(family)];
}

/** 把页面缩放交给外壳的 WebView；脱离外壳（浏览器预览 / 测试）时忽略。 */
export function applyUiZoom(scale: number) {
  const next = normalizeUiZoom(scale);
  void api
    .setUiZoom(next)
    .then(() => {
      // 缩放改了可视区域大小，催一下各栏重新按新宽度适配。
      window.dispatchEvent(new Event("resize"));
    })
    .catch(() => {
      // 外壳不在（浏览器里预览、跑测试）就跳过，不拦界面。
    });
}

/** 启动时按本地偏好先上字号、字体样式与缩放，避免首帧不对。 */
export function applyStoredUiScale() {
  applyUiFont(readStoredUiFont());
  applyUiFontFamily(readStoredUiFontFamily());
  applyUiZoom(readStoredUiZoom());
}

/** 写缩放比例：落盘 + 立即生效 + 通知同窗口其它组件。 */
export function writeStoredUiZoom(value: number) {
  const next = normalizeUiZoom(value);
  try {
    window.localStorage.setItem(UI_ZOOM_KEY, String(next));
  } catch {
    // 存不下只影响下次打开，当前照样生效。
  }
  applyUiZoom(next);
  window.dispatchEvent(new CustomEvent(UI_SCALE_EVENT, { detail: { zoom: next } }));
}

/** 写基准字号：落盘 + 立即生效 + 通知同窗口其它组件。 */
export function writeStoredUiFont(value: number) {
  const next = normalizeUiFont(value);
  try {
    window.localStorage.setItem(UI_FONT_KEY, String(next));
  } catch {
    // 存不下只影响下次打开，当前照样生效。
  }
  applyUiFont(next);
  window.dispatchEvent(new CustomEvent(UI_SCALE_EVENT, { detail: { font: next } }));
}

/** 写字体样式：落盘 + 立即生效 + 通知同窗口其它组件。 */
export function writeStoredUiFontFamily(value: UiFontFamily) {
  const next = normalizeUiFontFamily(value);
  try {
    window.localStorage.setItem(UI_FONT_FAMILY_KEY, next);
  } catch {
    // 存不下只影响下次打开，当前照样生效。
  }
  applyUiFontFamily(next);
  window.dispatchEvent(new CustomEvent(UI_SCALE_EVENT, { detail: { fontFamily: next } }));
}

/** 在候选档位里往上 / 往下走一档；已到顶 / 到底就停住。 */
export function nextUiZoom(current: number, direction: 1 | -1): number {
  const level = normalizeUiZoom(current);
  if (direction > 0) {
    return UI_ZOOM_LEVELS.find((candidate) => candidate > level + 1e-6) ?? UI_ZOOM_MAX;
  }
  let next: number = UI_ZOOM_MIN;
  for (const candidate of UI_ZOOM_LEVELS) {
    if (candidate < level - 1e-6) next = candidate;
  }
  return next;
}

/** 设置页和快捷键共用的状态：当前缩放、字号、字体样式，以及改它们的方法。 */
export function useUiScale() {
  const [zoom, setZoomState] = useState<number>(() => readStoredUiZoom());
  const [font, setFontState] = useState<number>(() => readStoredUiFont());
  const [fontFamily, setFontFamilyState] = useState<UiFontFamily>(() => readStoredUiFontFamily());

  useEffect(() => {
    const sync = () => {
      const storedZoom = readStoredUiZoom();
      const storedFont = readStoredUiFont();
      const storedFamily = readStoredUiFontFamily();
      applyUiFont(storedFont);
      applyUiFontFamily(storedFamily);
      setZoomState(storedZoom);
      setFontState(storedFont);
      setFontFamilyState(storedFamily);
    };
    const onChanged = (
      event: Event,
    ) => {
      const detail =
        (event as CustomEvent<{ zoom?: unknown; font?: unknown; fontFamily?: unknown }>).detail ??
        {};
      if (typeof detail.zoom === "number") setZoomState(normalizeUiZoom(detail.zoom));
      if (typeof detail.font === "number") setFontState(normalizeUiFont(detail.font));
      if (detail.fontFamily !== undefined) {
        setFontFamilyState(normalizeUiFontFamily(detail.fontFamily));
      }
    };
    window.addEventListener("storage", sync);
    window.addEventListener(UI_SCALE_EVENT, onChanged);
    return () => {
      window.removeEventListener("storage", sync);
      window.removeEventListener(UI_SCALE_EVENT, onChanged);
    };
  }, []);

  const setZoom = useCallback((value: number) => {
    writeStoredUiZoom(value);
    setZoomState(normalizeUiZoom(value));
  }, []);

  const setFont = useCallback((value: number) => {
    writeStoredUiFont(value);
    setFontState(normalizeUiFont(value));
  }, []);

  const setFontFamily = useCallback((value: UiFontFamily) => {
    writeStoredUiFontFamily(value);
    setFontFamilyState(normalizeUiFontFamily(value));
  }, []);

  return { zoom, font, fontFamily, setZoom, setFont, setFontFamily };
}

/**
 * 全局快捷键：Ctrl / ⌘ + 加号、减号 调页面缩放，Ctrl / ⌘ + 0 复位。
 * 返回解绑函数；正常启动不需要解绑，测试里用来还原。
 */
export function installUiZoomShortcuts(): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
    let next: number | undefined;
    if (event.key === "+" || event.key === "=") next = nextUiZoom(readStoredUiZoom(), 1);
    else if (event.key === "-" || event.key === "_") next = nextUiZoom(readStoredUiZoom(), -1);
    else if (event.key === "0") next = UI_ZOOM_DEFAULT;
    if (next === undefined) return;
    event.preventDefault();
    writeStoredUiZoom(next);
  };
  window.addEventListener("keydown", onKeyDown);
  return () => window.removeEventListener("keydown", onKeyDown);
}