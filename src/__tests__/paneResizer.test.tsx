//! 拖动条回归：无障碍属性、方向键步长、Home/End、双击复位和松手落盘。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import PaneResizer from "../PaneResizer";

beforeAll(() => {
  // jsdom 不带指针捕获，补齐后用事件验证拖动流程。
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
});

afterEach(cleanup);

function setup(overrides: Partial<ComponentProps<typeof PaneResizer>> = {}) {
  const onChange = vi.fn();
  const onCommit = vi.fn();
  const onReset = vi.fn();
  render(
    <PaneResizer
      label="邮件列表宽度"
      value={420}
      min={300}
      max={560}
      onChange={onChange}
      onCommit={onCommit}
      onReset={onReset}
      {...overrides}
    />,
  );
  const separator = screen.getByRole("separator", { name: "邮件列表宽度" });
  return { onChange, onCommit, onReset, separator };
}

describe("拖动条", () => {
  it("带可读名称、方向与数值范围，并可聚焦", () => {
    const { separator } = setup();
    expect(separator.getAttribute("aria-orientation")).toBe("vertical");
    expect(separator.getAttribute("aria-valuenow")).toBe("420");
    expect(separator.getAttribute("aria-valuemin")).toBe("300");
    expect(separator.getAttribute("aria-valuemax")).toBe("560");
    expect(separator.getAttribute("tabindex")).toBe("0");
  });

  it("左右方向键每次 16 像素，调整后立即落盘", () => {
    const { onChange, onCommit, separator } = setup();
    fireEvent.keyDown(separator, { key: "ArrowRight" });
    expect(onChange).toHaveBeenLastCalledWith(436);
    fireEvent.keyDown(separator, { key: "ArrowLeft" });
    expect(onChange).toHaveBeenLastCalledWith(404);
    expect(onCommit).toHaveBeenCalledTimes(2);
  });

  it("按住 Shift 时每次 48 像素", () => {
    const { onChange, separator } = setup();
    fireEvent.keyDown(separator, { key: "ArrowRight", shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith(468);
    fireEvent.keyDown(separator, { key: "ArrowLeft", shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith(372);
  });

  it("Home 与 End 直接跳到最小和最大", () => {
    const { onChange, separator } = setup();
    fireEvent.keyDown(separator, { key: "Home" });
    expect(onChange).toHaveBeenLastCalledWith(300);
    fireEvent.keyDown(separator, { key: "End" });
    expect(onChange).toHaveBeenLastCalledWith(560);
  });

  it("拖动中只改宽度，松手后才落盘", () => {
    const { onChange, onCommit, separator } = setup();
    fireEvent.pointerDown(separator, { button: 0, pointerId: 1, clientX: 400 });
    fireEvent.pointerMove(separator, { pointerId: 1, clientX: 460 });
    expect(onChange).toHaveBeenLastCalledWith(480);
    expect(onCommit).not.toHaveBeenCalled();
    fireEvent.pointerUp(separator, { pointerId: 1 });
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("双击复位", () => {
    const { onReset, separator } = setup();
    fireEvent.doubleClick(separator);
    expect(onReset).toHaveBeenCalledTimes(1);
  });
});