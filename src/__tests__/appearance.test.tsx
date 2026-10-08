//! 外观设置：深色模式要作用在整个界面，不只是读信窗格。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import AppearanceSettingsPanel from "../AppearanceSettingsPanel";
import { READER_THEME_KEY, applyStoredTheme } from "../readerTheme";

beforeEach(() => {
  window.localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.removeProperty("color-scheme");
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.removeProperty("color-scheme");
});

describe("外观设置", () => {
  it("默认跟随系统，不往根节点写死主题", () => {
    render(<AppearanceSettingsPanel />);

    const select = screen.getByLabelText("深色模式") as HTMLSelectElement;
    expect(select.value).toBe("auto");
    expect(document.documentElement.getAttribute("data-theme")).toBeNull();
    expect(screen.queryByText(/整个界面和读信窗格一起换色/)).toBeNull();
    expect(screen.queryByText(/改完立即生效/)).toBeNull();
    expect(screen.queryByRole("button", { name: "保存设置" })).toBeNull();
  });

  it("选深色后整个界面的根节点都切到深色", () => {
    render(<AppearanceSettingsPanel />);

    fireEvent.change(screen.getByLabelText("深色模式"), { target: { value: "dark" } });

    expect(window.localStorage.getItem(READER_THEME_KEY)).toBe("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });

  it("选浅色会写死浅色，压住系统深色", () => {
    render(<AppearanceSettingsPanel />);

    fireEvent.change(screen.getByLabelText("深色模式"), { target: { value: "light" } });

    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(document.documentElement.style.colorScheme).toBe("light");
  });

  it("已存过的偏好会在启动时应用", () => {
    window.localStorage.setItem(READER_THEME_KEY, "dark");

    applyStoredTheme();

    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("已存偏好会回显到下拉框", () => {
    window.localStorage.setItem(READER_THEME_KEY, "light");

    render(<AppearanceSettingsPanel />);

    expect((screen.getByLabelText("深色模式") as HTMLSelectElement).value).toBe("light");
  });
});