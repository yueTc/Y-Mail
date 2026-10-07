//! 写信富文本编辑器的回归：插入菜单、字体 / 字号 / 颜色、插图与粘贴。
//!
//! 这里渲染真的 TipTap 编辑器，只把 Tauri 命令层、文件对话框与事件通道换成桩。

import { open } from "@tauri-apps/plugin-dialog";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api";
import RichTextEditor from "../RichTextEditor";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    readInlineImage: vi.fn(),
    saveInlineImage: vi.fn(),
    openScreenshotOverlay: vi.fn(),
    takeScreenshotPreview: vi.fn(),
    finishScreenshot: vi.fn(),
    cancelScreenshot: vi.fn(),
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

/** 一遍真的编辑器 + 把最新 HTML 打到页面上，方便断言。 */
function Harness({ initial = "" }: { initial?: string }) {
  const [html, setHtml] = useState(initial);
  return (
    <>
      <RichTextEditor value={html} onChange={setHtml} onAttach={() => {}} />
      <output data-testid="html">{html}</output>
    </>
  );
}

/** 等编辑器挂载：正文区是带 aria-label 的可编辑区。 */
async function ready(): Promise<HTMLElement> {
  return screen.findByLabelText("正文");
}

/** 全选正文；TipTap 的格式命令要有选区才看得出来。 */
function selectAll(content: HTMLElement) {
  content.focus();
  fireEvent.keyDown(content, { key: "a", ctrlKey: true });
}

function currentHtml(): string {
  return screen.getByTestId("html").textContent ?? "";
}

/** 造一张已经读好的图片信息。 */
function imageInfo(filename = "a.png") {
  return {
    path: `C:\\tmp\\${filename}`,
    filename,
    mimeType: "image/png",
    dataUrl: "data:image/png;base64,AAAA",
    bytes: 3,
  };
}

beforeEach(() => {
  vi.mocked(api.readInlineImage).mockResolvedValue(imageInfo());
  vi.mocked(api.saveInlineImage).mockResolvedValue(imageInfo("粘贴.png"));
  vi.mocked(api.openScreenshotOverlay).mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("写信正文工具栏", () => {
  it("字号菜单能改字号", async () => {
    render(<Harness initial="<p>你好</p>" />);
    const content = await ready();
    selectAll(content);

    fireEvent.click(screen.getByRole("button", { name: "字号 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "24" }));

    await waitFor(() => expect(currentHtml()).toContain("font-size: 24px"));
  });

  it("字体菜单能改字体", async () => {
    render(<Harness initial="<p>你好</p>" />);
    const content = await ready();
    selectAll(content);

    fireEvent.click(screen.getByRole("button", { name: "字体 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "微软雅黑" }));

    await waitFor(() => expect(currentHtml()).toContain("微软雅黑"));
  });

  it("颜色面板能上色并记进最近使用", async () => {
    render(<Harness initial="<p>你好</p>" />);
    const content = await ready();
    selectAll(content);

    fireEvent.click(screen.getByRole("button", { name: "颜色" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "颜色 #FF0000" }));

    // 浏览器会把 #FF0000 规范化成 rgb()，这里按规范化后的写法断言。
    await waitFor(() => expect(currentHtml()).toContain("color: rgb(255, 0, 0)"));
    // 再开一次面板，「最近使用」里应该有刚才那个颜色。
    fireEvent.click(screen.getByRole("button", { name: "颜色" }));
    expect(screen.getByRole("menuitem", { name: "最近使用 #FF0000" })).toBeTruthy();
  });

  it("加粗 / 斜体 / 下划线 / 删除线都能开关", async () => {
    render(<Harness initial="<p>文字</p>" />);
    const content = await ready();
    selectAll(content);

    fireEvent.click(screen.getByRole("button", { name: "加粗" }));
    await waitFor(() => expect(currentHtml()).toContain("<strong>"));
    fireEvent.click(screen.getByRole("button", { name: "斜体" }));
    await waitFor(() => expect(currentHtml()).toContain("<em>"));
    fireEvent.click(screen.getByRole("button", { name: "下划线" }));
    await waitFor(() => expect(currentHtml()).toContain("<u>"));
    fireEvent.click(screen.getByRole("button", { name: "删除线" }));
    await waitFor(() => expect(currentHtml()).toContain("<s>"));
  });

  it("背景色面板能上底色", async () => {
    render(<Harness initial="<p>文字</p>" />);
    const content = await ready();
    selectAll(content);

    fireEvent.click(screen.getByRole("button", { name: "背景色" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "背景色 #FFC000" }));

    await waitFor(() =>
      expect(currentHtml()).toContain("background-color: rgb(255, 192, 0)"),
    );
  });

  it("对齐能选居左 / 居中 / 居右", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "居中" }));
    await waitFor(() => expect(currentHtml()).toContain("text-align: center"));
    fireEvent.click(screen.getByRole("button", { name: "居右" }));
    await waitFor(() => expect(currentHtml()).toContain("text-align: right"));
    fireEvent.click(screen.getByRole("button", { name: "居左" }));
    await waitFor(() => expect(currentHtml()).toContain("text-align: left"));
  });

  it("能切项目符号列表与数字列表", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "项目符号列表" }));
    await waitFor(() => expect(currentHtml()).toContain("<ul"));
    // 再点一次取消，然后换数字列表。
    fireEvent.click(screen.getByRole("button", { name: "项目符号列表" }));
    fireEvent.click(screen.getByRole("button", { name: "数字编号列表" }));
    await waitFor(() => expect(currentHtml()).toContain("<ol"));
  });

  it("缩进能加一级、再减一级", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "增加缩进" }));
    await waitFor(() => expect(currentHtml()).toContain("margin-left: 24px"));
    fireEvent.click(screen.getByRole("button", { name: "增加缩进" }));
    await waitFor(() => expect(currentHtml()).toContain("margin-left: 48px"));
    fireEvent.click(screen.getByRole("button", { name: "减少缩进" }));
    await waitFor(() => expect(currentHtml()).toContain("margin-left: 24px"));
  });

  it("行距能加一档、减一档", async () => {
    render(<Harness />);
    await ready();

    // 没设过时按 1.5 算，加一档到 1.75。
    fireEvent.click(screen.getByRole("button", { name: "增加行距" }));
    await waitFor(() => expect(currentHtml()).toContain("line-height: 1.75"));
    fireEvent.click(screen.getByRole("button", { name: "减少行距" }));
    await waitFor(() => expect(currentHtml()).toContain("line-height: 1.5"));
    fireEvent.click(screen.getByRole("button", { name: "减少行距" }));
    await waitFor(() => expect(currentHtml()).toContain("line-height: 1.15"));
  });

  it("插入表格先问几行几列", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "插入 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "表格" }));
    fireEvent.change(screen.getByLabelText("表格行数"), { target: { value: "3" } });
    fireEvent.change(screen.getByLabelText("表格列数"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "插入" }));

    await waitFor(() => expect(currentHtml()).toContain("<table"));
    expect(currentHtml()).toContain("<th");
    expect(currentHtml().match(/<tr/g)?.length).toBe(3);
  });

  it("插入链接先问文字和地址，缺协议自动补 https", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "插入 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "链接" }));
    fireEvent.change(screen.getByLabelText("链接文字"), { target: { value: "示例" } });
    fireEvent.change(screen.getByLabelText("链接地址"), { target: { value: "example.com/a" } });
    fireEvent.click(screen.getByRole("button", { name: "插入" }));

    await waitFor(() => expect(currentHtml()).toContain('href="https://example.com/a"'));
    expect(currentHtml()).toContain("示例");
  });

  it("危险协议的链接不插进去", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "插入 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "链接" }));
    fireEvent.change(screen.getByLabelText("链接地址"), { target: { value: "javascript:alert(1)" } });
    fireEvent.click(screen.getByRole("button", { name: "插入" }));

    expect(await screen.findByText(/链接要以/)).toBeTruthy();
    expect(currentHtml()).not.toContain("javascript:");
  });

  it("从资源管理器选图片后插进正文，并带上本地路径", async () => {
    vi.mocked(open).mockResolvedValue("C:\\tmp\\a.png");
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "插入 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "图片" }));

    await waitFor(() => expect(currentHtml()).toContain("<img"));
    expect(currentHtml()).toContain("data-local-path");
    expect(vi.mocked(api.readInlineImage)).toHaveBeenCalledWith("C:\\tmp\\a.png");
  });

  it("粘贴图片会先落盘，再插进正文", async () => {
    render(<Harness />);
    const content = await ready();
    const file = new File([new Uint8Array([1, 2, 3])], "clip.png", { type: "image/png" });

    fireEvent.paste(content, {
      clipboardData: {
        files: [file],
        items: [],
        types: ["Files"],
        getData: () => "",
      },
    });

    await waitFor(() => expect(vi.mocked(api.saveInlineImage)).toHaveBeenCalled());
    await waitFor(() => expect(currentHtml()).toContain("<img"));
    expect(currentHtml()).toContain("data-local-path");
  });

  it("拖拽经过时正文区加高亮类，附件列在工具栏下面", async () => {
    const { container } = render(
      <RichTextEditor
        value=""
        onChange={() => {}}
        onAttach={() => {}}
        attachments={[{ path: "C:\\tmp\\a.pdf", filename: "a.pdf" }]}
        dropActive
      />,
    );
    await ready();

    const body = container.querySelector(".rte-editor");
    expect(body?.className).toContain("drop-active");
    expect(screen.getByText("a.pdf")).toBeTruthy();
  });

  it("清除格式去掉加粗与颜色，链接留着", async () => {
    render(
      <Harness initial={'<p><strong>粗</strong><a href="https://example.com">链</a></p>'} />,
    );
    const content = await ready();
    selectAll(content);

    fireEvent.click(screen.getByRole("button", { name: "清除格式" }));

    await waitFor(() => expect(currentHtml()).not.toContain("<strong>"));
    expect(currentHtml()).toContain('href="https://example.com"');
  });

  it("格式刷先复制格式，选区一变就刷上并自动失效", async () => {
    render(<Harness initial="<p><strong>粗</strong>普通</p>" />);
    const content = await ready();

    const brush = screen.getByRole("button", { name: "格式刷" });
    fireEvent.click(brush);
    expect(brush.getAttribute("aria-pressed")).toBe("true");

    // 全选：选区变大了，抬键时把复制来的加粗刷上去，然后自动失效。
    content.focus();
    fireEvent.keyDown(content, { key: "a", ctrlKey: true });
    fireEvent.keyUp(content, { key: "a", ctrlKey: true });

    await waitFor(() => expect(brush.getAttribute("aria-pressed")).toBe("false"));
    expect(currentHtml()).toContain("<strong>粗普通</strong>");
  });

  it("格式刷与清除格式是图标按钮，鼠标悬停能看到名字", async () => {
    render(<Harness />);
    await ready();

    for (const name of ["格式刷", "清除格式"]) {
      const button = screen.getByRole("button", { name });
      expect(button.getAttribute("title")).toBe(name);
      expect(button.querySelector("svg")).not.toBeNull();
    }
  });

  it("截图按钮会打开截图", async () => {
    render(<Harness />);
    await ready();

    fireEvent.click(screen.getByRole("button", { name: "插入 ▾" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "截图" }));

    await waitFor(() => expect(vi.mocked(api.openScreenshotOverlay)).toHaveBeenCalled());
  });
});