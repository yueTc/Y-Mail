//! 三栏宽度计算回归：预算、取整、上下限、压缩顺序和极端窄屏。

import { describe, expect, it } from "vitest";

import {
  COMPOSE_MIN_WIDTH,
  CONTACTS_PANE_PROFILE,
  DEFAULT_PANE_WIDTHS,
  INBOX_PANE_PROFILE,
  LIST_LIMITS,
  READER_MIN_WIDTH,
  RESIZER_WIDTH,
  RAIL_WIDTH,
  SIDEBAR_LIMITS,
  fitPaneWidths,
  paneBudget,
} from "../usePaneWidths";

describe("三栏宽度计算", () => {
  it("预算扣掉设置栏、两条拖动条和邮件内容最小宽度", () => {
    expect(paneBudget(1440)).toBe(1440 - RAIL_WIDTH - RESIZER_WIDTH * 2 - READER_MIN_WIDTH);
  });

  it("写信时按 480 像素给内容区留位置", () => {
    expect(paneBudget(1440, COMPOSE_MIN_WIDTH)).toBe(
      1440 - RAIL_WIDTH - RESIZER_WIDTH * 2 - COMPOSE_MIN_WIDTH,
    );
  });

  it("空间够时只做取整，不动宽度", () => {
    expect(fitPaneWidths({ sidebar: 240.4, list: 420.6 }, 800)).toEqual({
      sidebar: 240,
      list: 421,
    });
  });

  it("超过上限时分别压到最大宽度", () => {
    expect(fitPaneWidths({ sidebar: 999, list: 999 }, 2000)).toEqual({
      sidebar: SIDEBAR_LIMITS.max,
      list: LIST_LIMITS.max,
    });
  });

  it("低于下限时分别抬到最小宽度", () => {
    expect(fitPaneWidths({ sidebar: 1, list: 1 }, 2000)).toEqual({
      sidebar: SIDEBAR_LIMITS.min,
      list: LIST_LIMITS.min,
    });
  });

  it("空间不够时先挤邮件列表，再挤邮箱栏", () => {
    expect(fitPaneWidths({ sidebar: 240, list: 560 }, 500)).toEqual({
      sidebar: 200,
      list: 300,
    });
  });

  it("极窄屏按比例收缩，优先保住邮件内容栏", () => {
    const result = fitPaneWidths(DEFAULT_PANE_WIDTHS, 240);
    expect(result.sidebar + result.list).toBeLessThanOrEqual(240);
    expect(result.sidebar).toBeLessThan(SIDEBAR_LIMITS.min);
    expect(result.list).toBeLessThan(LIST_LIMITS.min);
  });

  it("非法或负预算不产生负数", () => {
    expect(fitPaneWidths({ sidebar: Number.NaN, list: Number.NaN }, 800)).toEqual({
      sidebar: SIDEBAR_LIMITS.min,
      list: LIST_LIMITS.min,
    });
    expect(fitPaneWidths(DEFAULT_PANE_WIDTHS, -100)).toEqual({ sidebar: 0, list: 0 });
  });
});

describe("通讯录档位", () => {
  it("和收件箱共用一套逻辑，但存储键与默认值各有一套", () => {
    expect(CONTACTS_PANE_PROFILE.storageKey).not.toBe(INBOX_PANE_PROFILE.storageKey);
    expect(CONTACTS_PANE_PROFILE.defaults).not.toEqual(INBOX_PANE_PROFILE.defaults);
    expect(INBOX_PANE_PROFILE.storageKey).toBe("ymail.pane-widths.v1");
    expect(INBOX_PANE_PROFILE.defaults).toEqual(DEFAULT_PANE_WIDTHS);
    expect(INBOX_PANE_PROFILE.sidebarLimits).toEqual(SIDEBAR_LIMITS);
    expect(INBOX_PANE_PROFILE.listLimits).toEqual(LIST_LIMITS);
  });

  it("窗口变窄时通讯录也先挤列表栏，再挤分组栏", () => {
    const wide = { sidebar: 220, list: 400 };
    expect(fitPaneWidths(wide, 700, CONTACTS_PANE_PROFILE)).toEqual({ sidebar: 220, list: 400 });

    const tight = fitPaneWidths(wide, 500, CONTACTS_PANE_PROFILE);
    expect(tight.list).toBe(300); // 先把列表压到下限
    expect(tight.sidebar).toBe(200); // 还不够才挤分组栏
  });

  it("超过档位上限时按档位收口", () => {
    const over = fitPaneWidths({ sidebar: 900, list: 900 }, 5000, CONTACTS_PANE_PROFILE);
    expect(over).toEqual({ sidebar: 320, list: 560 });
  });
});