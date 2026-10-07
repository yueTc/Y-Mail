//! 截图叠加窗的回归：选区换算、拖框取图、Esc 取消。

import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api";
import ScreenshotOverlay, { areaFrom } from "../ScreenshotOverlay";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    takeScreenshotPreview: vi.fn(),
    finishScreenshot: vi.fn(),
    cancelScreenshot: vi.fn(),
  },
}));

describe("选区换算", () => {
  it("往右下拖，按起点到终点算", () => {
    expect(areaFrom({ x: 10, y: 20 }, { x: 40, y: 60 })).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
  });

  it("往左上拖，宽高同样是正的", () => {
    expect(areaFrom({ x: 40, y: 60 }, { x: 10, y: 20 })).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
  });

  it("还没开始拖就没有选区", () => {
    expect(areaFrom(undefined, { x: 1, y: 1 })).toBeNull();
    expect(areaFrom({ x: 1, y: 1 }, undefined)).toBeNull();
  });
});

describe("截图叠加窗", () => {
  beforeEach(() => {
    vi.mocked(api.takeScreenshotPreview).mockResolvedValue("data:image/jpeg;base64,AAAA");
    vi.mocked(api.finishScreenshot).mockResolvedValue(undefined);
    vi.mocked(api.cancelScreenshot).mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("拖好一块区域后把坐标交给后端（按物理像素）", async () => {
    const { container } = render(<ScreenshotOverlay />);
    const overlay = container.querySelector(".screenshot-overlay") as HTMLElement;
    await waitFor(() => expect(vi.mocked(api.takeScreenshotPreview)).toHaveBeenCalled());

    fireEvent.mouseDown(overlay, { button: 0, clientX: 10, clientY: 20 });
    fireEvent.mouseMove(overlay, { clientX: 110, clientY: 70 });
    fireEvent.mouseUp(overlay);

    await waitFor(() =>
      expect(vi.mocked(api.finishScreenshot)).toHaveBeenCalledWith(10, 20, 100, 50),
    );
  });

  it("按 Esc 取消截图", async () => {
    render(<ScreenshotOverlay />);
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(vi.mocked(api.cancelScreenshot)).toHaveBeenCalled());
  });

  it("太小的一划不当成截图", async () => {
    const { container } = render(<ScreenshotOverlay />);
    const overlay = container.querySelector(".screenshot-overlay") as HTMLElement;

    fireEvent.mouseDown(overlay, { button: 0, clientX: 10, clientY: 10 });
    fireEvent.mouseUp(overlay);

    expect(vi.mocked(api.finishScreenshot)).not.toHaveBeenCalled();
  });
});