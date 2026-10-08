//! 界面语言：只存「中文 / English」到本地，不碰邮件数据，也不发任何网络请求。
//!
//! 文案以中文原文当键：界面上原样写中文，`t("收件箱")` 在中文模式下原样返回，
//! 在英文模式下查对照表换成英文。查不到就先用中文兜底，绝不吞掉内容。
//! 切换语言会广播一个本地事件，根组件订阅后整棵树重渲染，不用重启。

import { useCallback, useEffect, useState } from "react";

import { CONTEXT_SEPARATOR, EN } from "./i18n.en";

/** 支持的界面语言。 */
export type Language = "zh" | "en";

/** 本地存储键：只存界面语言。 */
export const LANGUAGE_KEY = "ymail.language";

/** 同一窗口里改语言后通知其它组件刷新；浏览器自带的 storage 事件只在别的窗口触发。 */
const LANGUAGE_EVENT = "ymail:language-change";

/** 只认这两档，别的一律当没设置过。 */
function normalizeLanguage(value: string | null | undefined): Language | undefined {
  if (value === "zh" || value === "en") return value;
  return undefined;
}

/** 读本地存储里的语言偏好；隐私模式下拿不到就用中文。 */
export function readStoredLanguage(): Language {
  try {
    return normalizeLanguage(window.localStorage.getItem(LANGUAGE_KEY)) ?? "zh";
  } catch {
    return "zh";
  }
}

/** 当前界面语言；`t` 每次调用都读它，切换语言后立刻生效。 */
let currentLanguage: Language = readStoredLanguage();

/** 当前语言；编辑器里拿到的是最近一次生效值。 */
export function getLanguage(): Language {
  return currentLanguage;
}

/** 启动时按本地偏好先定一次语言，首帧就是对的。 */
export function applyStoredLanguage() {
  currentLanguage = readStoredLanguage();
}

/** 把 `{0}`、`{1}` 这样的占位符换成实参；没给参数就原样返回。 */
function substitute(text: string, params?: ReadonlyArray<string | number>): string {
  if (!params || params.length === 0) return text;
  return text.replace(/\{(\d+)\}/g, (match, index) => {
    const value = params[Number(index)];
    return value === undefined ? match : String(value);
  });
}

/**
 * 把界面上的中文文案翻成当前语言。
 *
 * - 中文模式：原样返回（只是把占位符填上）；
 * - 英文模式：查对照表；查不到就先用中文兜底。
 * - 同一个中文词在别处意思不同（「关闭」在 AI 选项里是 Off、在弹窗按钮上是 Close），
 *   就用第三个参数指明上下文；中文模式完全不受影响。
 */
export function t(
  zh: string,
  params?: ReadonlyArray<string | number>,
  context?: string,
): string {
  if (currentLanguage === "zh") return substitute(zh, params);
  const en =
    context === undefined ? EN[zh] : EN[context + CONTEXT_SEPARATOR + zh] ?? EN[zh];
  return substitute(en === undefined ? zh : en, params);
}

/** 写语言偏好并通知同窗口的其它组件；存储不可用时只影响下次打开。 */
export function setLanguage(value: Language) {
  currentLanguage = value;
  try {
    window.localStorage.setItem(LANGUAGE_KEY, value);
  } catch {
    // 本地存储不可用时忽略，界面照常当场切换。
  }
  window.dispatchEvent(new CustomEvent(LANGUAGE_EVENT, { detail: value }));
}

/** 设置页和根组件共用的语言状态：[当前语言, 切换]。 */
export function useLanguage(): [Language, (value: Language) => void] {
  const [language, setLanguageState] = useState<Language>(() => currentLanguage);

  useEffect(() => {
    const onStorage = () => {
      currentLanguage = readStoredLanguage();
      setLanguageState(currentLanguage);
    };
    const onChanged = (event: Event) => {
      const detail = (event as CustomEvent<unknown>).detail;
      const next = normalizeLanguage(typeof detail === "string" ? detail : null) ?? readStoredLanguage();
      currentLanguage = next;
      setLanguageState(next);
    };
    window.addEventListener("storage", onStorage);
    window.addEventListener(LANGUAGE_EVENT, onChanged);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener(LANGUAGE_EVENT, onChanged);
    };
  }, []);

  const update = useCallback((value: Language) => {
    setLanguage(value);
    setLanguageState(value);
  }, []);

  return [language, update];
}