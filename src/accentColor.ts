//! 主色调偏好：只存「用哪个强调色」到本地，不碰邮件数据，也不发任何网络请求。
//!
//! 强调色在界面上叫主色调，走 CSS 变量 `--accent` / `--accent-fg`：
//! - 浅色模式用 `--accent-light` / `--accent-fg-light`；
//! - 深色模式用 `--accent-dark` / `--accent-fg-dark`。
//! index.css 给这四个变量留了默认值，这里把它们写到 <html> 上，整套界面跟着换色。
//! 深色模式下把主色调往白色方向提亮，保证压在深底上看得清。

import { useCallback, useEffect, useState } from "react";

/** 本地存储键：只存主色调。 */
export const ACCENT_KEY = "ymail.accent-color.v1";

/** 主色调写在 <html> 上的四个 CSS 变量名。 */
export const ACCENT_LIGHT_VAR = "--accent-light";
export const ACCENT_LIGHT_FG_VAR = "--accent-fg-light";
export const ACCENT_DARK_VAR = "--accent-dark";
export const ACCENT_DARK_FG_VAR = "--accent-fg-dark";

/** 预设主色调编号；存本地时也存这个编号。 */
export type AccentPresetId = "blue" | "green" | "violet" | "orange" | "rose";

/** 五个预设主色调；`label` 是中文名，显示时过一遍 `t()`；深色值是提亮后的版本。 */
export const ACCENT_PRESETS: readonly {
  id: AccentPresetId;
  label: string;
  light: string;
  dark: string;
}[] = [
  { id: "blue", label: "蓝色", light: "#2f6fed", dark: "#6b9dff" },
  { id: "green", label: "绿色", light: "#16a34a", dark: "#67c98c" },
  { id: "violet", label: "紫色", light: "#7c3aed", dark: "#aa7ff3" },
  { id: "orange", label: "橙色", light: "#ea580c", dark: "#f19261" },
  { id: "rose", label: "玫红", light: "#db2777", dark: "#e873a7" },
];

/** 没设置过时用的预设。 */
export const ACCENT_DEFAULT: AccentPresetId = "blue";

/** 深色 / 浅色正文色：`--accent-fg` 只在这两个里挑。 */
const DARK_TEXT = "#10131a";
const LIGHT_TEXT = "#ffffff";

/** 同一窗口里改设置后通知其它组件刷新；浏览器自带的 storage 事件只在别的窗口触发。 */
const ACCENT_EVENT = "ymail:accent-color-change";

/** 是不是五个预设之一。 */
export function isPresetId(value: string): value is AccentPresetId {
  return ACCENT_PRESETS.some((preset) => preset.id === value);
}

/** 认 `#rrggbb`（大小写都行），别的返回空。 */
function normalizeHex(value: string): string | undefined {
  const match = /^#([0-9a-fA-F]{6})$/.exec(value.trim());
  if (!match) return undefined;
  return `#${match[1].toLowerCase()}`;
}

/**
 * 只认「五个预设编号」或 `#rrggbb`；别的一律当没设置过，回落到默认蓝色。
 * 返回值要么是预设编号，要么是规范化过的十六进制色值。
 */
export function normalizeAccent(value: string | null | undefined): string {
  if (typeof value === "string") {
    if (isPresetId(value)) return value;
    const hex = normalizeHex(value);
    if (hex) return hex;
  }
  return ACCENT_DEFAULT;
}

/** 读本地存的主色调；读不到就用默认蓝色。 */
export function readStoredAccent(): string {
  try {
    return normalizeAccent(window.localStorage.getItem(ACCENT_KEY));
  } catch {
    return ACCENT_DEFAULT;
  }
}

/** 十六进制色值拆成三个 0–255 的分量。 */
function toRgb(hex: string): [number, number, number] {
  const value = normalizeHex(hex) ?? "#000000";
  return [
    Number.parseInt(value.slice(1, 3), 16),
    Number.parseInt(value.slice(3, 5), 16),
    Number.parseInt(value.slice(5, 7), 16),
  ];
}

/** 三个分量拼回 `#rrggbb`，超范围的夹回 0–255。 */
function toHex(rgb: [number, number, number]): string {
  const channel = (value: number) =>
    Math.max(0, Math.min(255, Math.round(value))).toString(16).padStart(2, "0");
  return `#${rgb.map(channel).join("")}`;
}

/** 相对亮度，0 是黑、1 是白；用来决定压白字还是深字。 */
function luminance(hex: string): number {
  const linear = toRgb(hex).map((value) => {
    const channel = value / 255;
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

/** 深色模式用的主色调：往白色方向提亮三成半，压在深底上才看得清。 */
export function liftForDark(hex: string): string {
  const [r, g, b] = toRgb(hex);
  const lift = (value: number) => value + 0.35 * (255 - value);
  return toHex([lift(r), lift(g), lift(b)]);
}

/** 浅色模式：够亮就给深字，正常色给白字。 */
function foregroundForLight(hex: string): string {
  return luminance(hex) > 0.19 ? DARK_TEXT : LIGHT_TEXT;
}

/** 深色模式的主色调普遍提亮过，多半配深字。 */
function foregroundForDark(hex: string): string {
  return luminance(hex) > 0.25 ? DARK_TEXT : LIGHT_TEXT;
}

/** 把选择解析成浅色 / 深色两套主色调和各自的前景色。 */
export function resolveAccent(choice: string): {
  light: string;
  dark: string;
  lightFg: string;
  darkFg: string;
} {
  const preset = ACCENT_PRESETS.find((item) => item.id === choice);
  if (preset) {
    return {
      light: preset.light,
      dark: preset.dark,
      lightFg: foregroundForLight(preset.light),
      darkFg: foregroundForDark(preset.dark),
    };
  }
  const light = normalizeHex(choice) ?? ACCENT_PRESETS[0].light;
  const dark = liftForDark(light);
  return {
    light,
    dark,
    lightFg: foregroundForLight(light),
    darkFg: foregroundForDark(dark),
  };
}

/** 把选择写到 <html> 的四个变量上；index.css 里浅色 / 深色两套令牌各取一个。 */
export function applyAccent(choice: string) {
  const { light, dark, lightFg, darkFg } = resolveAccent(normalizeAccent(choice));
  const root = document.documentElement;
  root.style.setProperty(ACCENT_LIGHT_VAR, light);
  root.style.setProperty(ACCENT_LIGHT_FG_VAR, lightFg);
  root.style.setProperty(ACCENT_DARK_VAR, dark);
  root.style.setProperty(ACCENT_DARK_FG_VAR, darkFg);
}

/** 启动时按本地偏好先上一次色，界面首帧就是对的。 */
export function applyStoredAccent() {
  applyAccent(readStoredAccent());
}

/** 写主色调并通知同窗口的其它组件；存储不可用时只影响下次打开，不影响当前界面。 */
export function writeStoredAccent(value: string) {
  const next = normalizeAccent(value);
  try {
    window.localStorage.setItem(ACCENT_KEY, next);
  } catch {
    // 本地存储不可用时忽略，界面照常当场换色。
  }
  applyAccent(next);
  window.dispatchEvent(new CustomEvent(ACCENT_EVENT, { detail: next }));
}

/** 设置页共用的主色调状态：[当前选择, 改选择]。 */
export function useAccentColor(): [string, (value: string) => void] {
  const [choice, setChoice] = useState<string>(() => readStoredAccent());

  useEffect(() => {
    const sync = () => {
      const stored = readStoredAccent();
      applyAccent(stored);
      setChoice(stored);
    };
    const onChanged = (event: Event) => {
      const detail = (event as CustomEvent<unknown>).detail;
      setChoice(normalizeAccent(typeof detail === "string" ? detail : null));
    };
    // 组件挂载时也补一次，保证不管从哪条路进来界面颜色都是对的。
    applyAccent(readStoredAccent());
    window.addEventListener("storage", sync);
    window.addEventListener(ACCENT_EVENT, onChanged);
    return () => {
      window.removeEventListener("storage", sync);
      window.removeEventListener(ACCENT_EVENT, onChanged);
    };
  }, []);

  const update = useCallback((value: string) => {
    writeStoredAccent(value);
    setChoice(normalizeAccent(value));
  }, []);

  return [choice, update];
}