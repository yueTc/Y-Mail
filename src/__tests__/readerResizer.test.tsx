//! 上下拖动条回归：方向、无障碍属性、拖拽方向、键盘步长和复位。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import ReaderResizer from "../ReaderResizer";

beforeAll(() => {
  // jsdom 不带指针捕获，补齐后用事件验证拖动流程。
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
});

afterEach(cleanup);

function setup(overrides: Partial<ComponentProps<typeof ReaderResizer>> = {}) {
  const onChange = vi.fn();
  const onCommit = vi.fn();
  const onReset = vi.fn();
  render(
    <ReaderResizer
      label="附件区高度"
      value={220}
      min={96}
      max={520}
      onChange={onChange}
      onCommit={onCommit}
      onReset={onReset}
      {...overrides}
    />,
  );
  const separator = screen.getByRole("separator", { name: "附件区高度" });
  return { onChange, onCommit, onReset, separator };
}

describe("上下拖动条", () => {
  it("带水平方向、可读名称和数值范围", () => {
    const { separator } = setup();
    expect(separator.getAttribute("aria-orientation")).toBe("horizontal");
    expect(separator.getAttribute("aria-valuenow")).toBe("220");
    expect(separator.getAttribute("aria-valuemin")).toBe("96");
    expect(separator.getAttribute("aria-valuemax")).toBe("520");
    expect(separator.getAttribute("tabindex")).toBe("0");
  });

  it("向上拖附件变高，向下拖附件变矮，松手才落盘", () => {
    const { onChange, onCommit, separator } = setup();
    fireEvent.pointerDown(separator, { button: 0, pointerId: 1, clientY: 400 });
    fireEvent.pointerMove(separator, { pointerId: 1, clientY: 340 });
    expect(onChange).toHaveBeenLastCalledWith(280);
    fireEvent.pointerMove(separator, { pointerId: 1, clientY: 460 });
    expect(onChange).toHaveBeenLastCalledWith(160);
    expect(onCommit).not.toHaveBeenCalled();
    fireEvent.pointerUp(separator, { pointerId: 1 });
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("上下方向键每次 12 像素，按住 Shift 为 36 像素，并立即落盘", () => {
    const { onChange, onCommit, separator } = setup();
    fireEvent.keyDown(separator, { key: "ArrowUp" });
    expect(onChange).toHaveBeenLastCalledWith(232);
    fireEvent.keyDown(separator, { key: "ArrowDown" });
    expect(onChange).toHaveBeenLastCalledWith(208);
    fireEvent.keyDown(separator, { key: "ArrowUp", shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith(256);
    fireEvent.keyDown(separator, { key: "ArrowDown", shiftKey: true });
    expect(onChange).toHaveBeenLastCalledWith(184);
    expect(onCommit).toHaveBeenCalledTimes(4);
  });

  it("Home 回默认高度，End 到最大高度，双击复位", () => {
    const { onReset, onChange, separator } = setup();
    fireEvent.keyDown(separator, { key: "Home" });
    expect(onReset).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(separator, { key: "End" });
    expect(onChange).toHaveBeenLastCalledWith(520);
    fireEvent.doubleClick(separator);
    expect(onReset).toHaveBeenCalledTimes(2);
  });
});