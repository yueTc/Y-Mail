//! 新建账号色标推荐：先用空缺色，再用占用最少的颜色。

import { describe, expect, it } from "vitest";

import { ACCOUNT_COLOR_PALETTE, recommendAccountColor } from "../accountColor";

describe("账号色标推荐", () => {
  it("没有已用颜色时从色板第一项开始", () => {
    expect(recommendAccountColor([])).toBe(ACCOUNT_COLOR_PALETTE[0]);
  });

  it("优先跳过已经使用的颜色，且不区分大小写", () => {
    expect(recommendAccountColor(["#3B82F6", "#10b981"])).toBe("#f59e0b");
  });

  it("色板全部占用时选出现次数最少的颜色", () => {
    const used = [...ACCOUNT_COLOR_PALETTE, "#3b82f6", "#10b981"];
    expect(recommendAccountColor(used)).toBe("#f59e0b");
  });

  it("空串和无关的自定义颜色不会占掉推荐位", () => {
    expect(recommendAccountColor(["", "#123456"])).toBe("#3b82f6");
  });
});
