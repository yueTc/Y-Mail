//! 红旗前端回归：按钮无障碍与不冒泡、列表乐观切换、读信页一致、失败提示。
//!
//! 虚拟滚动在 jsdom 里拿不到真实尺寸，这里把 `@tanstack/react-virtual` 换成
//! 「全部渲染」的桩，方便断言列表和读信页里的红旗按钮。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import FlagButton from "../FlagButton";
import InboxPanel from "../InboxPanel";
import type { InboxMessage, InboxThread, MessageBody } from "../api";
import { api } from "../api";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({
    count,
    estimateSize,
  }: {
    count: number;
    estimateSize: (index: number) => number;
  }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * estimateSize(index),
        size: estimateSize(index),
      })),
    getTotalSize: () => count * 68,
  }),
}));

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listInboxThreads: vi.fn(),
    listInboxMessages: vi.fn(),
    listInboxFolders: vi.fn(),
    inboxSummary: vi.fn(),
    listAiProviders: vi.fn(),
    syncStatus: vi.fn(),
    startSync: vi.fn(),
    stopSync: vi.fn(),
    setMessageRead: vi.fn(),
    setMessageFlagged: vi.fn(),
    searchMessages: vi.fn(),
    listThreadMessages: vi.fn(),
    getMessageBody: vi.fn(),
    downloadAttachment: vi.fn(),
    rememberRemoteSender: vi.fn(),
    listTrustedRemoteSenders: vi.fn(),
    forgetRemoteSender: vi.fn(),
  },
}));

function message(overrides: Partial<InboxMessage> = {}): InboxMessage {
  return {
    id: 1,
    accountId: 1,
    folderId: 10,
    uid: 1,
    threadKey: "t1",
    subject: "主题 1",
    fromName: "李四",
    fromAddr: "li@example.com",
    dateUtc: "2026-10-04T02:00:00Z",
    size: 1000,
    hasAttachments: false,
    isRead: true,
    isFlagged: false,
    snippet: "",
    accountEmail: "a@example.com",
    accountName: "测试账号",
    accountColor: "#3366ff",
    folderPath: "INBOX",
    ...overrides,
  };
}

const MESSAGE = message();
const THREAD: InboxThread = {
  accountId: 1,
  threadKey: "t1",
  messageCount: 1,
  unreadCount: 0,
  latest: MESSAGE,
};

const BODY: MessageBody = {
  messageId: 1,
  textPlain: "正文",
  html: "<p>正文</p>",
  blockedRemoteImages: 0,
  attachments: [],
};

/** 让面板进入「平铺邮件」模式：不点会话聚合。 */
async function setupPanel(flagResult: { changed: boolean; synced: boolean }) {
  vi.mocked(api.listInboxThreads).mockResolvedValue({ items: [THREAD], total: 1, offset: 0, limit: 200 });
  vi.mocked(api.listInboxMessages).mockResolvedValue({ items: [MESSAGE], total: 1, offset: 0, limit: 200 });
  vi.mocked(api.listInboxFolders).mockResolvedValue([]);
  vi.mocked(api.inboxSummary).mockResolvedValue({ accounts: [], totalUnread: 0, totalMessages: 0 });
  vi.mocked(api.listAiProviders).mockResolvedValue([]);
  vi.mocked(api.syncStatus).mockResolvedValue([]);
  vi.mocked(api.setMessageRead).mockResolvedValue(undefined);
  vi.mocked(api.setMessageFlagged).mockResolvedValue(flagResult);
  vi.mocked(api.getMessageBody).mockResolvedValue(BODY);

  render(<InboxPanel />);
  // 会话聚合默认关闭，列表天然平铺，每封邮件都是可打开的行。
  await screen.findByText("主题 1");
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(cleanup);

describe("红旗按钮", () => {
  it("是原生按钮、状态可读，回车不会冒泡给列表行", () => {
    const onToggle = vi.fn();
    const onRowKeyDown = vi.fn();
    render(
      <div onKeyDown={onRowKeyDown}>
        <FlagButton flagged={false} onToggle={onToggle} />
      </div>,
    );
    const button = screen.getByRole("button", { name: "标红" });
    expect(button.getAttribute("aria-pressed")).toBe("false");
    fireEvent.keyDown(button, { key: "Enter" });
    expect(onRowKeyDown).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("已标红时显示实心并给出取消标红的标签", () => {
    render(<FlagButton flagged onToggle={() => {}} />);
    const button = screen.getByRole("button", { name: "取消标红" });
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(button.textContent).toBe("★");
  });
});

describe("统一收件箱红旗闭环", () => {
  it("列表点一下立刻标红，读信页同步显示已标红", async () => {
    await setupPanel({ changed: true, synced: true });

    // 先打开邮件，让读信页出现红旗按钮。
    fireEvent.click(screen.getByText("主题 1").closest(".inbox-row")!);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "标红这封邮件" })).toBeTruthy(),
    );

    // 点列表里的红旗，本地立即变红。
    fireEvent.click(screen.getByRole("button", { name: "标红" }));
    expect(api.setMessageFlagged).toHaveBeenCalledWith(1, true);

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "取消标红这封邮件" })).toBeTruthy(),
    );
    // 列表里那一个也变成「取消标红」。
    expect(screen.getByRole("button", { name: "取消标红" })).toBeTruthy();
    // 只是切换红旗，不该触发行打开带来的已读逻辑。
    expect(api.setMessageRead).not.toHaveBeenCalled();
  });

  it("服务器回写失败时本地保持标红并给出可读提示", async () => {
    await setupPanel({ changed: true, synced: false });
    fireEvent.click(screen.getByRole("button", { name: "标红" }));
    await waitFor(() =>
      expect(
        screen.getByText("已在本机标红，服务器同步失败，稍后会自动重试"),
      ).toBeTruthy(),
    );
    expect(screen.getByRole("button", { name: "取消标红" })).toBeTruthy();
  });
});