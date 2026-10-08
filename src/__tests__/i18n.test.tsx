//! 界面语言：默认中文；切到英文后查对照表；查不到回退中文。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import AppearanceSettingsPanel from "../AppearanceSettingsPanel";
import { LANGUAGE_KEY, applyStoredLanguage, getLanguage, setLanguage, t } from "../i18n";

beforeEach(() => {
  window.localStorage.clear();
  applyStoredLanguage();
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  applyStoredLanguage();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.removeProperty("color-scheme");
});

describe("界面语言", () => {
  it("默认是中文，原文原样返回", () => {
    expect(getLanguage()).toBe("zh");
    expect(t("收件箱")).toBe("收件箱");
  });

  it("切到英文后走对照表", () => {
    setLanguage("en");

    expect(getLanguage()).toBe("en");
    expect(t("收件箱")).toBe("Inbox");
  });

  it("占位符会按实参填上", () => {
    expect(t("共 {0} {1}", [3, "封"])).toBe("共 3 封");

    setLanguage("en");

    expect(t("共 {0} {1}", [3, "封"])).toBe("3 封 in total");
  });

  it("同一个词按上下文取不同译文", () => {
    setLanguage("en");

    expect(t("关闭")).toBe("Close");
    expect(t("关闭", undefined, "ai")).toBe("Off");
    expect(t("主题")).toBe("Theme");
    expect(t("主题", undefined, "subject")).toBe("Subject");
  });

  it("查不到就回退中文，不吞内容", () => {
    setLanguage("en");

    expect(t("这一句肯定没有英文")).toBe("这一句肯定没有英文");
  });

  it("中文模式下上下文参数不影响结果", () => {
    expect(t("关闭", undefined, "ai")).toBe("关闭");
  });

  it("设置里能选语言，选完存本地并当场生效", () => {
    render(<AppearanceSettingsPanel />);

    fireEvent.change(screen.getByLabelText("界面语言"), { target: { value: "en" } });

    expect(window.localStorage.getItem(LANGUAGE_KEY)).toBe("en");
    expect(getLanguage()).toBe("en");
    expect(screen.getByLabelText("Language")).toBeTruthy();
  });
});