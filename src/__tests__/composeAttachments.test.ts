//! 写信附件纯函数的回归：路径转附件、去重合并、拖拽命中判断。
//!
//! 这里不碰界面，只验「选择文件」和「拖拽」两条路共用的那套逻辑。

import { describe, expect, it } from "vitest";

import {
  basename,
  mergeAttachments,
  pathsToAttachments,
  pointInsideRect,
  toCssPoint,
} from "../composeAttachments";

describe("附件路径转附件项", () => {
  it("Windows 与正斜杠路径都能取出文件名", () => {
    expect(basename("C:\\tmp\\报告.pdf")).toBe("报告.pdf");
    expect(basename("/home/me/report.pdf")).toBe("report.pdf");
    expect(basename("C:\\tmp\\folder\\")).toBe("folder");
    expect(basename("   ")).toBe("");
  });

  it("去空、去首尾空格、按路径去重且保持顺序", () => {
    const list = pathsToAttachments([
      " C:\\tmp\\b.pdf ",
      "",
      "C:\\tmp\\a.png",
      "C:\\tmp\\b.pdf",
      "   ",
    ]);
    expect(list).toEqual([
      { path: "C:\\tmp\\b.pdf", filename: "b.pdf" },
      { path: "C:\\tmp\\a.png", filename: "a.png" },
    ]);
  });
});

describe("附件列表合并", () => {
  it("已在列表里的路径不再重复加，先有的项优先", () => {
    const current = [{ path: "C:\\tmp\\a.png", filename: "a.png" }];
    const merged = mergeAttachments(current, [
      { path: "C:\\tmp\\a.png", filename: "冒充同名.png" },
      { path: "C:\\tmp\\b.pdf", filename: "b.pdf" },
    ]);
    expect(merged).toEqual([
      { path: "C:\\tmp\\a.png", filename: "a.png" },
      { path: "C:\\tmp\\b.pdf", filename: "b.pdf" },
    ]);
    // 不改原数组。
    expect(current).toHaveLength(1);
  });
});

describe("拖拽落点判断", () => {
  const rect = { left: 100, top: 50, right: 500, bottom: 400 };

  it("缩放为 2 时物理像素折半再比", () => {
    expect(toCssPoint({ x: 300, y: 200 }, 2)).toEqual({ x: 150, y: 100 });
    // 比例拿不到时按 1 算，不能崩。
    expect(toCssPoint({ x: 300, y: 200 }, 0)).toEqual({ x: 300, y: 200 });
  });

  it("只认落在窗格矩形里的点", () => {
    expect(pointInsideRect({ x: 150, y: 100 }, rect)).toBe(true);
    expect(pointInsideRect({ x: 100, y: 50 }, rect)).toBe(true);
    expect(pointInsideRect({ x: 99, y: 100 }, rect)).toBe(false);
    expect(pointInsideRect({ x: 150, y: 401 }, rect)).toBe(false);
  });
});