//! 写信加附件两个入口的回归：资源管理器选择、拖进写信窗格。
//!
//! 渲染真实 ComposePanel，只把 Tauri 命令层、文件拖拽订阅、系统文件对话框换成桩。
//! 拖到窗格外面不生效，同一个文件重复拖不重复挂。

import { open } from "@tauri-apps/plugin-dialog";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AccountInboxSummary, Signature } from "../api";
import { api } from "../api";
import ComposePanel from "../ComposePanel";
import { subscribeFileDrop, type WindowFileDropEvent } from "../fileDrop";

vi.mock("../RichTextEditor");

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    composeDraft: vi.fn(),
    saveDraft: vi.fn(),
    enqueueOutbox: vi.fn(),
    retryOutbox: vi.fn(),
    listOutbox: vi.fn(),
    getOutbox: vi.fn(),
    deleteOutbox: vi.fn(),
    searchContacts: vi.fn(),
    getSignature: vi.fn(),
    saveSignature: vi.fn(),
    sendOutbox: vi.fn(),
  },
}));

vi.mock("../fileDrop", () => ({
  isTauriRuntime: () => true,
  subscribeFileDrop: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

const ACCOUNTS: AccountInboxSummary[] = [
  {
    accountId: 1,
    email: "sender@example.com",
    displayName: "测试账号",
    color: "#3366ff",
    enabled: true,
    messageCount: 10,
    unreadCount: 2,
  },
];

const EMPTY_SIGNATURE: Signature = {
  accountId: 1,
  html: "",
  enabled: false,
  updatedAt: "2026-10-04T00:00:00Z",
};

/** 窗格矩形：800x600，从左上角起算，测试里按这个判断落点。 */
function stubRect() {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    left: 0,
    top: 0,
    right: 800,
    bottom: 600,
    width: 800,
    height: 600,
    toJSON: () => ({}),
  } as DOMRect);
}

let dropHandler: ((event: WindowFileDropEvent) => void) | undefined;

function renderPanel() {
  return render(<ComposePanel request={{ kind: "new" }} accounts={ACCOUNTS} onClose={() => {}} />);
}

async function waitForReady() {
  await waitFor(() => {
    const select = screen.getByLabelText("发信账号") as HTMLSelectElement;
    expect(select.value).toBe("1");
  });
}

/** 模拟外壳推来一次拖拽事件。 */
function fireDrop(event: WindowFileDropEvent) {
  act(() => {
    dropHandler?.(event);
  });
}

beforeEach(() => {
  dropHandler = undefined;
  vi.mocked(subscribeFileDrop).mockImplementation((handler) => {
    dropHandler = handler;
    return () => {};
  });
  stubRect();
  vi.mocked(api.listOutbox).mockResolvedValue([]);
  vi.mocked(api.getSignature).mockResolvedValue(EMPTY_SIGNATURE);
  vi.mocked(api.searchContacts).mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("写信附件入口", () => {
  it("点工具栏「附件」把资源管理器选中的路径加进附件列表", async () => {
    vi.mocked(open).mockResolvedValue(["C:\\tmp\\报告.pdf", "C:\\tmp\\img.png"]);
    renderPanel();
    await waitForReady();

    fireEvent.click(screen.getByRole("button", { name: "附件" }));

    expect(await screen.findByText("报告.pdf")).toBeTruthy();
    expect(screen.getByText("img.png")).toBeTruthy();
    expect(vi.mocked(open)).toHaveBeenCalledWith({ multiple: true, title: "选择附件" });
  });

  it("把文件拖进写信窗格就加上，重复拖同一个不重复挂", async () => {
    renderPanel();
    await waitForReady();

    fireDrop({ type: "drop", paths: ["C:\\tmp\\a.pdf"], position: { x: 100, y: 100 } });
    expect(await screen.findByText("a.pdf")).toBeTruthy();

    fireDrop({ type: "drop", paths: ["C:\\tmp\\a.pdf"], position: { x: 100, y: 100 } });
    expect(screen.getAllByText("a.pdf")).toHaveLength(1);
  });

  it("拖到写信窗格外面不加附件", async () => {
    renderPanel();
    await waitForReady();

    fireDrop({ type: "drop", paths: ["C:\\tmp\\out.pdf"], position: { x: 900, y: 100 } });
    expect(screen.queryByText("out.pdf")).toBeNull();
  });

  it("拖着文件经过窗格时正文区高亮，离开就灭掉", async () => {
    const { container } = renderPanel();
    await waitForReady();
    const body = container.querySelector(".rte-editor");

    fireDrop({ type: "enter", paths: ["C:\\tmp\\a.pdf"], position: { x: 100, y: 100 } });
    expect(body?.className).toContain("drop-active");

    fireDrop({ type: "leave", paths: [] });
    expect(body?.className).not.toContain("drop-active");
  });

  it("点「移除」能把附件摘掉", async () => {
    renderPanel();
    await waitForReady();

    fireDrop({ type: "drop", paths: ["C:\\tmp\\a.pdf"], position: { x: 100, y: 100 } });
    expect(await screen.findByText("a.pdf")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "移除附件 a.pdf" }));
    expect(screen.queryByText("a.pdf")).toBeNull();
  });
});