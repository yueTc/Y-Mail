//! 附件区高度回归：合法范围、窗口收缩、落盘、重开读取和复位。

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  ATTACHMENT_HEIGHT_KEY,
  DEFAULT_ATTACHMENT_HEIGHT,
  MAX_ATTACHMENT_HEIGHT,
  MIN_ATTACHMENT_HEIGHT,
  READER_RESIZER_HEIGHT,
  attachmentHeightLimits,
  useAttachmentHeight,
} from "../useAttachmentHeight";

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

describe("附件区高度范围", () => {
  it("空间充足时使用固定上下限", () => {
    expect(attachmentHeightLimits(1000)).toEqual({
      min: MIN_ATTACHMENT_HEIGHT,
      max: MAX_ATTACHMENT_HEIGHT,
    });
    expect(attachmentHeightLimits(400)).toEqual({ min: 96, max: 212 });
  });

  it("窗口很小时正文和附件都保留正高度", () => {
    for (const available of [120, 250]) {
      const limits = attachmentHeightLimits(available);
      const bodyHeight = available - READER_RESIZER_HEIGHT - limits.max;
      expect(limits.min).toBeGreaterThan(0);
      expect(limits.max).toBeGreaterThanOrEqual(limits.min);
      expect(bodyHeight).toBeGreaterThan(0);
    }
  });

  it("拿不到容器高度时回默认上下限", () => {
    expect(attachmentHeightLimits(undefined)).toEqual({
      min: MIN_ATTACHMENT_HEIGHT,
      max: MAX_ATTACHMENT_HEIGHT,
    });
  });
});

describe("附件区高度记忆", () => {
  it("读取记忆后先按当前窗口校正，不越界", () => {
    window.localStorage.setItem(ATTACHMENT_HEIGHT_KEY, "999");
    const { result } = renderHook(() => useAttachmentHeight(400));
    expect(result.current.height).toBe(212);
  });

  it("低于下限时抬到最小值", () => {
    window.localStorage.setItem(ATTACHMENT_HEIGHT_KEY, "1");
    const { result } = renderHook(() => useAttachmentHeight(400));
    expect(result.current.height).toBe(MIN_ATTACHMENT_HEIGHT);
  });

  it("本地记忆不是数字时回默认值", () => {
    window.localStorage.setItem(ATTACHMENT_HEIGHT_KEY, "not-a-number");
    const { result } = renderHook(() => useAttachmentHeight(600));
    expect(result.current.height).toBe(DEFAULT_ATTACHMENT_HEIGHT);
  });

  it("拖动中只改内存，落盘后重开仍是同一高度", () => {
    const first = renderHook(() => useAttachmentHeight(400));
    act(() => first.result.current.setHeight(160));
    expect(first.result.current.height).toBe(160);
    expect(window.localStorage.getItem(ATTACHMENT_HEIGHT_KEY)).toBeNull();
    act(() => first.result.current.commit());
    expect(window.localStorage.getItem(ATTACHMENT_HEIGHT_KEY)).toBe("160");

    first.unmount();
    const reopened = renderHook(() => useAttachmentHeight(400));
    expect(reopened.result.current.height).toBe(160);
  });

  it("窗口变小后显示高度收进新范围，但不覆盖用户记忆", () => {
    const { result, rerender } = renderHook(
      ({ available }: { available: number }) => useAttachmentHeight(available),
      { initialProps: { available: 1000 } },
    );
    act(() => result.current.commit(520));
    expect(result.current.height).toBe(520);

    rerender({ available: 400 });
    expect(result.current.height).toBe(212);
    expect(window.localStorage.getItem(ATTACHMENT_HEIGHT_KEY)).toBe("520");
  });

  it("Home 对应复位会回默认高度并落盘", () => {
    const { result } = renderHook(() => useAttachmentHeight(600));
    act(() => result.current.setHeight(96));
    act(() => result.current.commit());
    expect(result.current.height).toBe(96);
    act(() => result.current.reset());
    expect(result.current.height).toBe(DEFAULT_ATTACHMENT_HEIGHT);
    expect(window.localStorage.getItem(ATTACHMENT_HEIGHT_KEY)).toBe(String(DEFAULT_ATTACHMENT_HEIGHT));
  });
});
