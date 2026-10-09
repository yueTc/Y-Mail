//! 页面缩放、字体大小、字体样式：偏好读写、范围夹取、快捷键，以及外观页三个下拉。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AppearanceSettingsPanel from "../AppearanceSettingsPanel";
import { api } from "../api";
import {
  UI_FONT_FAMILY_KEY,
  UI_FONT_FAMILY_STACKS,
  UI_FONT_KEY,
  UI_ZOOM_KEY,
  applyStoredUiScale,
  installUiZoomShortcuts,
  nextUiZoom,
  readStoredUiFont,
  readStoredUiFontFamily,
  readStoredUiZoom,
  writeStoredUiFont,
  writeStoredUiFontFamily,
  writeStoredUiZoom,
} from "../uiScale";

vi.mock("../api", () => ({
  api: {
    setUiZoom: vi.fn().mockResolvedValue(undefined),
  },
}));

/** 拿到被 mock 的缩放命令，断言它收到什么比例。 */
const setUiZoomMock = api.setUiZoom as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => {
  window.localStorage.clear();
  document.documentElement.style.removeProperty("font-size");
  document.documentElement.style.removeProperty("font-family");
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.removeProperty("color-scheme");
  setUiZoomMock.mockClear();
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  document.documentElement.style.removeProperty("font-size");
  document.documentElement.style.removeProperty("font-family");
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.removeProperty("color-scheme");
});

describe("页面缩放、字体大小与字体样式", () => {
  it("默认 100% / 标准字号 / 系统默认字体", () => {
    expect(readStoredUiZoom()).toBe(1);
    expect(readStoredUiFont()).toBe(15);
    expect(readStoredUiFontFamily()).toBe("system");
  });

  it("存坏的值收进范围", () => {
    window.localStorage.setItem(UI_ZOOM_KEY, "9");
    expect(readStoredUiZoom()).toBe(1.5);
    window.localStorage.setItem(UI_ZOOM_KEY, "abc");
    expect(readStoredUiZoom()).toBe(1);
    window.localStorage.setItem(UI_FONT_KEY, "99");
    expect(readStoredUiFont()).toBe(19);
    window.localStorage.setItem(UI_FONT_KEY, "1");
    expect(readStoredUiFont()).toBe(13);
    // 不在白名单里的字体样式一律当没设置过。
    window.localStorage.setItem(UI_FONT_FAMILY_KEY, "comic-sans");
    expect(readStoredUiFontFamily()).toBe("system");
  });

  it("写缩放会落盘并交给外壳", () => {
    writeStoredUiZoom(1.25);
    expect(window.localStorage.getItem(UI_ZOOM_KEY)).toBe("1.25");
    expect(setUiZoomMock).toHaveBeenCalledWith(1.25);
  });

  it("写字号会落盘并改根节点基准字号", () => {
    writeStoredUiFont(17);
    expect(window.localStorage.getItem(UI_FONT_KEY)).toBe("17");
    expect(document.documentElement.style.fontSize).toBe("17px");
  });

  it("写字体样式会落盘并改根节点字体栈", () => {
    writeStoredUiFontFamily("simsun");
    expect(window.localStorage.getItem(UI_FONT_FAMILY_KEY)).toBe("simsun");
    expect(document.documentElement.style.fontFamily).toBe(UI_FONT_FAMILY_STACKS.simsun);
  });

  it("启动时按存过的偏好应用", () => {
    window.localStorage.setItem(UI_FONT_KEY, "19");
    window.localStorage.setItem(UI_FONT_FAMILY_KEY, "kaiti");
    window.localStorage.setItem(UI_ZOOM_KEY, "1.5");

    applyStoredUiScale();

    expect(document.documentElement.style.fontSize).toBe("19px");
    expect(document.documentElement.style.fontFamily).toBe(UI_FONT_FAMILY_STACKS.kaiti);
    expect(setUiZoomMock).toHaveBeenCalledWith(1.5);
  });

  it("快捷键 Ctrl + 加号 / 减号 / 0 调缩放", () => {
    const uninstall = installUiZoomShortcuts();
    try {
      fireEvent.keyDown(window, { key: "=", ctrlKey: true });
      expect(readStoredUiZoom()).toBe(1.1);
      fireEvent.keyDown(window, { key: "+", ctrlKey: true });
      expect(readStoredUiZoom()).toBe(1.25);
      fireEvent.keyDown(window, { key: "-", ctrlKey: true });
      expect(readStoredUiZoom()).toBe(1.1);
      fireEvent.keyDown(window, { key: "0", ctrlKey: true });
      expect(readStoredUiZoom()).toBe(1);
    } finally {
      uninstall();
    }
  });

  it("档位到顶 / 到底就不再走", () => {
    expect(nextUiZoom(1.5, 1)).toBe(1.5);
    expect(nextUiZoom(0.8, -1)).toBe(0.8);
  });

  it("外观页三个下拉，改动立即生效", () => {
    render(<AppearanceSettingsPanel />);

    const zoomSelect = screen.getByLabelText("页面缩放") as HTMLSelectElement;
    expect(zoomSelect.value).toBe("1");
    fireEvent.change(zoomSelect, { target: { value: "1.25" } });
    expect(window.localStorage.getItem(UI_ZOOM_KEY)).toBe("1.25");
    expect(setUiZoomMock).toHaveBeenCalledWith(1.25);

    const fontSelect = screen.getByLabelText("字体大小") as HTMLSelectElement;
    expect(fontSelect.value).toBe("15");
    // 字号用数字表示，不再是小 / 标准 / 大 / 特大。
    expect(screen.getByRole("option", { name: "15 px" })).toBeTruthy();
    expect(screen.queryByRole("option", { name: "标准" })).toBeNull();
    fireEvent.change(fontSelect, { target: { value: "17" } });
    expect(window.localStorage.getItem(UI_FONT_KEY)).toBe("17");
    expect(document.documentElement.style.fontSize).toBe("17px");

    const familySelect = screen.getByLabelText("字体样式") as HTMLSelectElement;
    expect(familySelect.value).toBe("system");
    fireEvent.change(familySelect, { target: { value: "simhei" } });
    expect(window.localStorage.getItem(UI_FONT_FAMILY_KEY)).toBe("simhei");
    expect(document.documentElement.style.fontFamily).toBe(UI_FONT_FAMILY_STACKS.simhei);
  });

  it("已存偏好会回显到下拉框", () => {
    window.localStorage.setItem(UI_ZOOM_KEY, "1.5");
    window.localStorage.setItem(UI_FONT_KEY, "13");
    window.localStorage.setItem(UI_FONT_FAMILY_KEY, "yahei");

    render(<AppearanceSettingsPanel />);

    expect((screen.getByLabelText("页面缩放") as HTMLSelectElement).value).toBe("1.5");
    expect((screen.getByLabelText("字体大小") as HTMLSelectElement).value).toBe("13");
    expect((screen.getByLabelText("字体样式") as HTMLSelectElement).value).toBe("yahei");
  });
});