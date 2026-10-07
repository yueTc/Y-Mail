//! 段落格式的纯逻辑回归：行距档位。

import { describe, expect, it } from "vitest";

import {
  DEFAULT_LINE_HEIGHT,
  LINE_HEIGHT_STEPS,
  MAX_INDENT,
  stepLineHeight,
} from "../composeBlockFormat";

describe("行距档位", () => {
  it("默认档是 1.5，且确实在档位表里", () => {
    expect(DEFAULT_LINE_HEIGHT).toBe(1.5);
    expect(LINE_HEIGHT_STEPS).toContain(DEFAULT_LINE_HEIGHT);
  });

  it("增加一档往上走", () => {
    expect(stepLineHeight(1.5, 1)).toBe(1.75);
    expect(stepLineHeight(1, 1)).toBe(1.15);
  });

  it("减少一档往下走", () => {
    expect(stepLineHeight(1.5, -1)).toBe(1.15);
  });

  it("已经到顶或到底就停在原地", () => {
    const top = LINE_HEIGHT_STEPS[LINE_HEIGHT_STEPS.length - 1];
    expect(stepLineHeight(top, 1)).toBe(top);
    expect(stepLineHeight(1, -1)).toBe(1);
  });

  it("缩进上限是个正整数级别", () => {
    expect(MAX_INDENT).toBeGreaterThan(0);
  });
});