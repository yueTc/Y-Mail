//! 主色调：五个预设加自己调色；换色只走本地偏好，深浅两套各取一个值。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import AppearanceSettingsPanel from "../AppearanceSettingsPanel";
import {
  ACCENT_DARK_VAR,
  ACCENT_DEFAULT,
  ACCENT_KEY,
  ACCENT_LIGHT_VAR,
  ACCENT_PRESETS,
  applyStoredAccent,
  liftForDark,
  normalizeAccent,
  readStoredAccent,
  resolveAccent,
  writeStoredAccent,
} from "../accentColor";

/** 读根节点上的某个自定义属性。 */
function cssVar(name: string): string {
  return document.documentElement.style.getPropertyValue(name).trim();
}

beforeEach(() => {
  window.localStorage.clear();
  for (const name of ["--accent-light", "--accent-fg-light", "--accent-dark", "--accent-fg-dark"]) {
    document.documentElement.style.removeProperty(name);
  }
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  for (const name of ["--accent-light", "--accent-fg-light", "--accent-dark", "--accent-fg-dark"]) {
    document.documentElement.style.removeProperty(name);
  }
});

describe("主色调", () => {
  it("默认是蓝色，读坏值也回退蓝色", () => {
    expect(readStoredAccent()).toBe(ACCENT_DEFAULT);
    window.localStorage.setItem(ACCENT_KEY, "紫色大花");
    expect(readStoredAccent()).toBe(ACCENT_DEFAULT);
    expect(normalizeAccent(null)).toBe(ACCENT_DEFAULT);
    expect(normalizeAccent("#ABCDEF")).toBe("#abcdef");
    expect(normalizeAccent("#12345")).toBe(ACCENT_DEFAULT);
  });

  it("五个预设各有一套浅色值和深色值", () => {
    expect(ACCENT_PRESETS.length).toBe(5);
    for (const preset of ACCENT_PRESETS) {
      const resolved = resolveAccent(preset.id);
      expect(resolved.light).toBe(preset.light);
      expect(resolved.dark).toBe(preset.dark);
      expect(resolved.lightFg).toMatch(/^#(ffffff|10131a)$/);
      expect(resolved.darkFg).toMatch(/^#(ffffff|10131a)$/);
    }
  });

  it("选预设会落盘并写到根节点四个变量上", () => {
    writeStoredAccent("green");

    expect(window.localStorage.getItem(ACCENT_KEY)).toBe("green");
    expect(cssVar(ACCENT_LIGHT_VAR)).toBe(resolveAccent("green").light);
    expect(cssVar(ACCENT_DARK_VAR)).toBe(resolveAccent("green").dark);
    // 前景色在浅色和深色下各写一份，保证按钮文字看得清。
    expect(cssVar("--accent-fg-light")).toMatch(/^#(ffffff|10131a)$/);
    expect(cssVar("--accent-fg-dark")).toMatch(/^#(ffffff|10131a)$/);
  });

  it("自己调色存的是色值，深色下自动提亮", () => {
    writeStoredAccent("#0b7285");

    expect(window.localStorage.getItem(ACCENT_KEY)).toBe("#0b7285");
    expect(cssVar(ACCENT_LIGHT_VAR)).toBe("#0b7285");
    expect(cssVar(ACCENT_DARK_VAR)).toBe(liftForDark("#0b7285"));
    // 提亮后的深色值必须比原色亮，不然深底上看不清。
    expect(cssVar(ACCENT_DARK_VAR)).not.toBe("#0b7285");
  });

  it("启动时按存过的偏好应用", () => {
    window.localStorage.setItem(ACCENT_KEY, "violet");

    applyStoredAccent();

    expect(cssVar(ACCENT_LIGHT_VAR)).toBe(resolveAccent("violet").light);
  });

  it("外观页有五个预设色块和一个自己调色", () => {
    render(<AppearanceSettingsPanel />);

    for (const preset of ACCENT_PRESETS) {
      expect(screen.getByLabelText(preset.label)).toBeTruthy();
    }
    expect(screen.getByLabelText("自己调色")).toBeTruthy();
  });

  it("点色块当场换色，选中的色块有标记", () => {
    render(<AppearanceSettingsPanel />);

    const green = screen.getByLabelText("绿色");
    expect(green.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(green);

    expect(window.localStorage.getItem(ACCENT_KEY)).toBe("green");
    expect(cssVar(ACCENT_LIGHT_VAR)).toBe(resolveAccent("green").light);
    expect(screen.getByLabelText("绿色").getAttribute("aria-pressed")).toBe("true");
  });

  it("用过自己调色后，色块都不再是选中态", () => {
    render(<AppearanceSettingsPanel />);

    fireEvent.change(screen.getByLabelText("自己调色"), { target: { value: "#ff0000" } });

    expect(window.localStorage.getItem(ACCENT_KEY)).toBe("#ff0000");
    expect(cssVar(ACCENT_LIGHT_VAR)).toBe("#ff0000");
    for (const preset of ACCENT_PRESETS) {
      expect(screen.getByLabelText(preset.label).getAttribute("aria-pressed")).toBe("false");
    }
  });

  it("存过的选择会回显", () => {
    window.localStorage.setItem(ACCENT_KEY, "rose");

    render(<AppearanceSettingsPanel />);

    expect(screen.getByLabelText("玫红").getAttribute("aria-pressed")).toBe("true");
    expect((screen.getByLabelText("自己调色") as HTMLInputElement).value).toBe(
      resolveAccent("rose").light,
    );
  });
});