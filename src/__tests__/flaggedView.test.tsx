//! 「红旗邮件」左侧入口回归：每个账号的收件箱下面多一条，点了按账号看标红邮件。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import InboxPanel from "../InboxPanel";
import type { Account, InboxFolder } from "../api";
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
    getTotalSize: () => count * 73,
  }),
}));

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listInboxThreads: vi.fn(),
    listAccounts: vi.fn(),
    listProxies: vi.fn(),
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

const ACCOUNT: Account = {
  id: 1,
  displayName: "测试账号",
  email: "a@example.com",
  authType: "password",
  username: "a@example.com",
  imap: { host: "imap.example.com", port: 993, security: "tls" },
  smtp: { host: "smtp.example.com", port: 465, security: "tls" },
  proxy: { mode: "inherit" },
  color: "#3366ff",
  enabled: true,
  oauthProvider: null,
  oauthClientId: "",
  hasCredential: true,
  createdAt: "2026-10-01T00:00:00Z",
  updatedAt: "2026-10-01T00:00:00Z",
};
function folder(overrides: Partial<InboxFolder> = {}): InboxFolder {
  return {
    accountId: 1,
    folderId: 10,
    fullPath: "INBOX",
    kind: "inbox",
    messageCount: 3,
    unreadCount: 2,
    ...overrides,
  };
}


beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.inboxSummary).mockResolvedValue({
    accounts: [
      {
        accountId: 1,
        email: "a@example.com",
        displayName: "测试账号",
        color: "#3366ff",
        enabled: true,
        messageCount: 3,
        unreadCount: 2,
      },
    ],
    totalUnread: 2,
    totalMessages: 3,
  });
  vi.mocked(api.listInboxFolders).mockResolvedValue([
    folder(),
    folder({ folderId: 11, fullPath: "垃圾邮件", kind: "junk", unreadCount: 0 }),
  ]);
  vi.mocked(api.listInboxMessages).mockResolvedValue({
    items: [],
    total: 0,
    offset: 0,
    limit: 200,
  });
  vi.mocked(api.listInboxThreads).mockResolvedValue({
    items: [],
    total: 0,
    offset: 0,
    limit: 200,
  });
  vi.mocked(api.listAiProviders).mockResolvedValue([]);
  vi.mocked(api.syncStatus).mockResolvedValue([]);
  vi.mocked(api.listAccounts).mockResolvedValue([ACCOUNT]);
  vi.mocked(api.listProxies).mockResolvedValue([]);
});

afterEach(cleanup);

/** 展开账号的文件夹列表，返回可见的文件夹按钮文字。 */
async function openAccount(): Promise<string[]> {
  render(<InboxPanel />);
  const accountButton = await screen.findByRole("button", { name: /测试账号/ });
  fireEvent.click(accountButton);
  const entries = await screen.findAllByRole("button");
  // 侧栏里带 sidebar-folder 的那些就是文件夹项。
  return entries
    .filter((button) => button.classList.contains("sidebar-folder"))
    .map((button) => (button.textContent ?? "").trim());
}

describe("红旗邮件入口", () => {
  it("排在账号收件箱下面，垃圾邮件之类的照旧跟在后面", async () => {
    const labels = await openAccount();
    expect(labels.length).toBeGreaterThanOrEqual(3);
    expect(labels[0]).toBe("收件箱2");
    expect(labels[1]).toBe("红旗邮件");
    expect(labels[2]).toBe("垃圾邮件");
  });

  it("点一下按账号只看标红邮件，并且不折会话", async () => {
    await openAccount();
    const threadsBefore = vi.mocked(api.listInboxThreads).mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: /红旗邮件/ }));

    await waitFor(() =>
      expect(api.listInboxMessages).toHaveBeenCalledWith(
        expect.objectContaining({ accountId: 1, flaggedOnly: true }),
      ),
    );
    // 红旗视图一律平铺：点了之后不再打线程接口。
    expect(vi.mocked(api.listInboxThreads).mock.calls.length).toBe(threadsBefore);
    const call = vi.mocked(api.listInboxMessages).mock.calls.at(-1)?.[0];
    expect(call?.folderId).toBeUndefined();
    // 会话聚合开关在这个视图里是禁用状态。
    expect((screen.getByLabelText("按会话聚合") as HTMLInputElement).disabled).toBe(true);
  });

  it("一封标红邮件都没有时，给出专门的说法", async () => {
    await openAccount();
    fireEvent.click(screen.getByRole("button", { name: /红旗邮件/ }));
    await waitFor(() =>
      expect(screen.getByText("还没有标红的邮件。在列表里点星星就能标红。")).toBeTruthy(),
    );
  });
});

describe("顶部统一区入口", () => {
  it("统一收件箱下面排着未读 / 红旗 / 草稿 / 已发送", async () => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });
    const labels = Array.from(document.querySelectorAll(".inbox-sidebar .sidebar-item")).map(
      (element) => (element.textContent ?? "").replace(/\d+$/, "").trim(),
    );
    expect(labels).toEqual([
      "统一收件箱",
      "所有未读",
      "所有红旗",
      "所有草稿",
      "所有已发送",
      "测试账号",
      "＋ 添加邮箱",
    ]);
  });

  it("点「所有草稿」按文件夹类型查，点「所有红旗」按红旗查", async () => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });

    // 会话聚合默认关闭，这里手动打开，验证草稿在聚合模式下走线程接口。
    fireEvent.click(screen.getByLabelText("按会话聚合"));
    fireEvent.click(screen.getByRole("button", { name: /所有草稿/ }));
    // 草稿不受红旗影响，会话聚合照用户开关来，所以走线程接口。
    await waitFor(() =>
      expect(api.listInboxThreads).toHaveBeenCalledWith(
        expect.objectContaining({ folderKind: "draft" }),
      ),
    );

    fireEvent.click(screen.getByRole("button", { name: /所有红旗/ }));
    await waitFor(() =>
      expect(api.listInboxMessages).toHaveBeenCalledWith(
        expect.objectContaining({ flaggedOnly: true }),
      ),
    );
    const last = vi.mocked(api.listInboxMessages).mock.calls.at(-1)?.[0];
    expect(last?.folderKind).toBeUndefined();
    // 红旗视图一律平铺，不再打线程接口。
    expect(vi.mocked(api.listInboxThreads).mock.calls.at(-1)?.[0]).not.toHaveProperty(
      "flaggedOnly",
    );
  });

  it("点「所有未读」只看未读，并把那个开关锁住", async () => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });

    // 先手动打开会话聚合，验证未读筛选在聚合模式下走线程接口。
    fireEvent.click(screen.getByLabelText("按会话聚合"));
    fireEvent.click(screen.getByRole("button", { name: /所有未读/ }));
    await waitFor(() =>
      expect(api.listInboxThreads).toHaveBeenCalledWith(
        expect.objectContaining({ unreadOnly: true }),
      ),
    );
    const checkbox = screen.getByLabelText("只看未读") as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    expect(checkbox.disabled).toBe(true);
  });
});
describe("左侧账号展开", () => {
  it("点一下展开文件夹，再点一下收起", async () => {
    render(<InboxPanel />);
    const accountButton = await screen.findByRole("button", { name: /测试账号/ });
    expect(document.querySelectorAll(".sidebar-folder").length).toBe(0);

    fireEvent.click(accountButton);
    expect(document.querySelectorAll(".sidebar-folder").length).toBeGreaterThan(0);

    fireEvent.click(accountButton);
    expect(document.querySelectorAll(".sidebar-folder").length).toBe(0);
  });

  it("多个账号可以同时展开", async () => {
    vi.mocked(api.inboxSummary).mockResolvedValue({
      accounts: [
        {
          accountId: 1,
          email: "a@example.com",
          displayName: "测试账号",
          color: "#3366ff",
          enabled: true,
          messageCount: 3,
          unreadCount: 2,
        },
        {
          accountId: 2,
          email: "b@example.com",
          displayName: "第二个账号",
          color: "#ff6633",
          enabled: true,
          messageCount: 1,
          unreadCount: 0,
        },
      ],
      totalUnread: 2,
      totalMessages: 4,
    });
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder(),
      folder({ accountId: 2, folderId: 20, fullPath: "INBOX", kind: "inbox", unreadCount: 0 }),
    ]);

    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /测试账号/ }));
    fireEvent.click(screen.getByRole("button", { name: /第二个账号/ }));

    // 两个账号的文件夹区同时挂在页面上，不再互相顶掉。
    expect(document.querySelectorAll(".sidebar-folders").length).toBe(2);
  });

  it("点未选中账号的文件夹，列表按那个账号过滤", async () => {
    vi.mocked(api.inboxSummary).mockResolvedValue({
      accounts: [
        {
          accountId: 1,
          email: "a@example.com",
          displayName: "测试账号",
          color: "#3366ff",
          enabled: true,
          messageCount: 3,
          unreadCount: 2,
        },
        {
          accountId: 2,
          email: "b@example.com",
          displayName: "第二个账号",
          color: "#ff6633",
          enabled: true,
          messageCount: 1,
          unreadCount: 0,
        },
      ],
      totalUnread: 2,
      totalMessages: 4,
    });
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder(),
      folder({ accountId: 2, folderId: 20, fullPath: "INBOX", kind: "inbox", unreadCount: 0 }),
    ]);

    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /测试账号/ }));
    fireEvent.click(screen.getByRole("button", { name: /第二个账号/ }));

    // 手动打开会话聚合，验证按账号 / 文件夹过滤时走线程接口。
    fireEvent.click(screen.getByLabelText("按会话聚合"));

    const secondAccountFolder = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".sidebar-folder"),
    ).find((button) => button.textContent?.includes("收件箱") && button.closest(".sidebar-account")?.textContent?.includes("第二个账号"));
    expect(secondAccountFolder).toBeTruthy();
    fireEvent.click(secondAccountFolder as HTMLButtonElement);

    await waitFor(() =>
      expect(api.listInboxThreads).toHaveBeenCalledWith(
        expect.objectContaining({ accountId: 2, folderId: 20 }),
      ),
    );
  });
});
describe("邮箱右键编辑", () => {
  it("右键账号出「编辑」，点一下弹出编辑弹窗", async () => {
    render(<InboxPanel />);
    const accountButton = await screen.findByRole("button", { name: /测试账号/ });

    fireEvent.contextMenu(accountButton);
    const menu = await screen.findByRole("menu", { name: "邮箱菜单" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "编辑" }));

    const dialog = await screen.findByRole("dialog", { name: "编辑邮箱" });
    expect(within(dialog).getAllByDisplayValue("a@example.com").length).toBeGreaterThan(0);
    expect(api.listAccounts).toHaveBeenCalled();
  });
});