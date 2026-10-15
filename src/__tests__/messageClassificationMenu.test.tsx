import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import MessageClassificationMenu from "../MessageClassificationMenu";
import type { LabelGroup } from "../api";

const groups: LabelGroup[] = [{
  id: 1, name: "内容分类", kind: "choice", enabled: true, confidenceThreshold: 0.75,
  scoreMin: null, scoreMax: null, sortOrder: 10,
  labels: [{ id: 2, groupId: 1, name: "账单", color: "blue", sortOrder: 10, isFallback: false }],
}];
const callbacks = { onSetValue: vi.fn(), onClearValue: vi.fn() };

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("手动分类子菜单", () => {
  it("读取中、无标签组和读取失败分别显示明确提示", () => {
    const { rerender } = render(<MessageClassificationMenu groups={[]} loading error={false} {...callbacks} />);
    fireEvent.mouseEnter(screen.getByRole("menuitem", { name: "手动分类" }));
    expect(screen.getByRole("status").textContent).toBe("加载中……");
    rerender(<MessageClassificationMenu groups={[]} loading={false} error={false} {...callbacks} />);
    expect(screen.getByRole("status").textContent).toBe("暂无标签组");
    rerender(<MessageClassificationMenu groups={[]} loading={false} error {...callbacks} />);
    expect(screen.getByRole("status").textContent).toBe("标签组读取失败，请重新打开菜单");
  });

  it("方向键逐层进入，左键返回上层并恢复焦点", () => {
    render(<MessageClassificationMenu groups={groups} loading={false} error={false} {...callbacks} />);
    const root = screen.getByRole("menuitem", { name: "手动分类" });
    root.focus();
    fireEvent.keyDown(root, { key: "ArrowRight" });
    const group = screen.getByRole("menuitem", { name: "内容分类" });
    expect(document.activeElement).toBe(group);
    fireEvent.keyDown(group, { key: "ArrowRight" });
    const label = screen.getByRole("menuitem", { name: "账单" });
    expect(document.activeElement).toBe(label);
    fireEvent.keyDown(label, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "清除本组结果" }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
    expect(document.activeElement).toBe(group);
    expect(screen.queryByRole("menuitem", { name: "账单" })).toBeNull();
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(document.activeElement).toBe(root);
    expect(root.getAttribute("aria-expanded")).toBe("false");
  });

  it("点击也能逐层展开，支持不用鼠标悬停的操作", () => {
    render(<MessageClassificationMenu groups={groups} loading={false} error={false} {...callbacks} />);
    fireEvent.click(screen.getByRole("menuitem", { name: "手动分类" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "内容分类" }));
    expect(screen.getByRole("menuitem", { name: "账单" })).toBeTruthy();
  });

  it("右下角打开时向左展开，并将底部限制在窗口内", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.tagName === "BUTTON" && this.textContent === "内容分类") {
        return { left: 650, right: 760, top: 500, bottom: 532, width: 110, height: 32 } as DOMRect;
      }
      return this.tagName === "BUTTON"
        ? { left: 900, right: 1010, top: 740, bottom: 772, width: 110, height: 32 } as DOMRect
        : { left: 0, right: 180, top: 0, bottom: 240, width: 180, height: 240 } as DOMRect;
    });
    render(<MessageClassificationMenu groups={groups} loading={false} error={false} {...callbacks} />);
    fireEvent.mouseEnter(screen.getByRole("menuitem", { name: "手动分类" }));
    const menu = screen.getByRole("menu", { name: "手动分类" });
    expect(Number.parseFloat(menu.style.left)).toBe(720);
    expect(Number.parseFloat(menu.style.top)).toBe(window.innerHeight - 240 - 8);
    fireEvent.mouseEnter(screen.getByRole("menuitem", { name: "内容分类" }));
    expect(screen.getByRole("menu", { name: "内容分类" }).style.left).toBe("470px");
  });
});
