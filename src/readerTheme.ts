//! 主题偏好：只存「跟随系统 / 浅色 / 深色」到本地，不碰任何敏感数据。
//!
//! 这份偏好既管读信窗格，也管整个界面：选中深色 / 浅色后，会把
//! <html data-theme="dark|light"> 写上去，index.css 里整套颜色令牌跟着切换；
//! 选「跟随系统」时不写属性，交给浏览器的 prefers-color-scheme 判断。

import { useCallback, useEffect, useState } from "react";

/** 主题偏好；三档存本地，只记界面偏好，不涉及任何敏感信息。 */
export type ReaderTheme = "auto" | "light" | "dark";

/** 本地存储键：只存「跟随系统 / 浅色 / 深色」。 */
export const READER_THEME_KEY = "ymail.reader-theme";

/** 实际生效的主题写在 <html> 上的属性名。 */
export const THEME_ATTRIBUTE = "data-theme";

/** 同一窗口里改设置后通知其它组件刷新；浏览器自带的 storage 事件只在别的窗口触发。 */
const READER_THEME_EVENT = "ymail:reader-theme-change";

/** 只认这三档，别的一律当没设置过。 */
function normalizeTheme(value: string | null | undefined): ReaderTheme | undefined {
  if (value === "light" || value === "dark" || value === "auto") return value;
  return undefined;
}

/** 读本地存储里的主题偏好；隐私模式下拿不到就用「跟随系统」。 */
export function readStoredReaderTheme(): ReaderTheme {
  try {
    return normalizeTheme(window.localStorage.getItem(READER_THEME_KEY)) ?? "auto";
  } catch {
    return "auto";
  }
}

/**
 * 把主题偏好落到 <html>：
 * - 深色 / 浅色：写死对应属性，整个界面（含读信窗格）一起换色；
 * - 跟随系统：删掉属性，交给 CSS 的 prefers-color-scheme，系统一变自动跟。
 */
export function applyTheme(theme: ReaderTheme) {
  const root = document.documentElement;
  if (theme === "auto") {
    root.removeAttribute(THEME_ATTRIBUTE);
    root.style.removeProperty("color-scheme");
    return;
  }
  root.setAttribute(THEME_ATTRIBUTE, theme);
  // 顺带告诉浏览器原生控件（下拉框、滚动条）该用深色还是浅色。
  root.style.colorScheme = theme;
}

/** 启动时按本地偏好先上一次色，避免深色用户先闪一下白底。 */
export function applyStoredTheme() {
  applyTheme(readStoredReaderTheme());
}

/** 写主题偏好并通知同窗口的其它组件；存储不可用时只影响下次打开，不影响当前界面。 */
export function writeStoredReaderTheme(value: ReaderTheme) {
  try {
    window.localStorage.setItem(READER_THEME_KEY, value);
  } catch {
    // 本地存储不可用时忽略，界面照常当场换色。
  }
  applyTheme(value);
  window.dispatchEvent(new CustomEvent(READER_THEME_EVENT, { detail: value }));
}

/** 设置页和读信窗格共用的主题状态：[当前选择, 改选择]。 */
export function useReaderTheme(): [ReaderTheme, (value: ReaderTheme) => void] {
  const [theme, setTheme] = useState<ReaderTheme>(() => readStoredReaderTheme());

  useEffect(() => {
    const sync = () => {
      const stored = readStoredReaderTheme();
      applyTheme(stored);
      setTheme(stored);
    };
    const onChanged = (event: Event) => {
      const detail = (event as CustomEvent<unknown>).detail;
      setTheme(normalizeTheme(typeof detail === "string" ? detail : null) ?? readStoredReaderTheme());
    };
    window.addEventListener("storage", sync);
    window.addEventListener(READER_THEME_EVENT, onChanged);
    return () => {
      window.removeEventListener("storage", sync);
      window.removeEventListener(READER_THEME_EVENT, onChanged);
    };
  }, []);

  const update = useCallback((value: ReaderTheme) => {
    writeStoredReaderTheme(value);
    setTheme(value);
  }, []);

  return [theme, update];
}

/** 跟随系统深色：监听系统主题变化。 */
export function useSystemDark(): boolean {
  const [systemDark, setSystemDark] = useState(false);

  useEffect(() => {
    const query = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!query) return;
    const update = () => setSystemDark(query.matches);
    update();
    query.addEventListener?.("change", update);
    return () => query.removeEventListener?.("change", update);
  }, []);

  return systemDark;
}

/** 把三档偏好合成「现在是不是深色」。 */
export function isDarkTheme(theme: ReaderTheme, systemDark: boolean): boolean {
  return theme === "dark" || (theme === "auto" && systemDark);
}