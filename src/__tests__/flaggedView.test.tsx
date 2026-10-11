//! 「红旗邮件」左侧入口回归：每个账号的收件箱下面多一条，点了按账号看标红邮件。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import InboxPanel from "../InboxPanel";
import type { Account, AppSettings, InboxFolder, InboxMessage } from "../api";
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
    previewClassifyMessages: vi.fn(),
    classifyMessages: vi.fn(),
    getAppSettings: vi.fn(),
    setClassifySkipManualConfirm: vi.fn(),
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

const SETTINGS: AppSettings = {
  dataDir: "",
  attachmentDir: "",
  notifyNewMail: true,
  notifyAiEnabled: false,
  blockRemoteImagesByDefault: true,
  minimizeToTrayOnClose: true,
  startMinimizedToTray: false,
  autoCheckUpdate: false,
  updateCheckIntervalHours: 24,
  syncPollIntervalSeconds: 60,
  classifyEnabled: false,
  classifyConfidencePercent: 75,
  classifySkipManualConfirm: false,
  defaultDataDir: "",
  defaultAttachmentDir: "",
  activeDataDir: "",
  activeAttachmentDir: "",
  firstRun: false,
};

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
    delimiter: "/",
    changeable: false,
    allowsChildren: false,
    childCreateBlocked: false,
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
describe("列表标签胶囊", () => {
  function labelledMessage(overrides: Partial<InboxMessage> = {}): InboxMessage {
    return {
      id: 1,
      accountId: 1,
      folderId: 10,
      uid: 1,
      threadKey: "t1",
      subject: "账单邮件",
      fromName: "张三",
      fromAddr: "z@example.com",
      dateUtc: "2026-10-04T02:00:00Z",
      size: 1000,
      hasAttachments: false,
      isRead: false,
      isFlagged: false,
      snippet: "",
      accountEmail: "a@example.com",
      accountName: "测试账号",
      accountColor: "#3366ff",
      folderPath: "INBOX",
      labelId: 2,
      labelName: "账单",
      labelColor: "orange",
      ...overrides,
    };
  }

  it("平铺行把胶囊放在时间前面，点一下按标签搜索", async () => {
    vi.mocked(api.listInboxMessages).mockResolvedValue({
      items: [labelledMessage()],
      total: 1,
      offset: 0,
      limit: 200,
    });
    vi.mocked(api.searchMessages).mockResolvedValue({
      items: [],
      total: 0,
      offset: 0,
      limit: 200,
      deepSynced: false,
      deepError: null,
    });
    render(<InboxPanel />);

    const chip = await screen.findByRole("button", { name: "账单" });
    expect(chip.getAttribute("data-label-color")).toBe("orange");
    const time = chip.nextElementSibling as HTMLElement | null;
    expect(time?.classList.contains("inbox-row-time")).toBe(true);

    fireEvent.click(chip);
    await waitFor(() => {
      const searchBox = screen.getByLabelText("搜索邮件") as HTMLInputElement;
      expect(searchBox.value).toBe("label:账单");
    });
    await waitFor(() =>
      expect(vi.mocked(api.searchMessages)).toHaveBeenCalledWith(
        expect.objectContaining({ raw: "label:账单" }),
      ),
    );
  });

  it("没有标签时不渲染胶囊", async () => {
    vi.mocked(api.listInboxMessages).mockResolvedValue({
      items: [labelledMessage({ labelId: null, labelName: null, labelColor: null })],
      total: 1,
      offset: 0,
      limit: 200,
    });
    render(<InboxPanel />);
    await screen.findByText("账单邮件");
    expect(screen.queryByRole("button", { name: "账单" })).toBeNull();
  });

  it("会话折叠行用最新一封的标签与颜色", async () => {
    vi.mocked(api.listInboxThreads).mockResolvedValue({
      items: [
        {
          accountId: 1,
          threadKey: "t1",
          messageCount: 2,
          unreadCount: 1,
          latest: labelledMessage({ labelId: 3, labelName: "通知", labelColor: "teal" }),
        },
      ],
      total: 1,
      offset: 0,
      limit: 200,
    });
    render(<InboxPanel />);
    fireEvent.click(await screen.findByLabelText("按会话聚合"));

    const chip = await screen.findByRole("button", { name: "通知" });
    expect(chip.getAttribute("data-label-color")).toBe("teal");
  });

  it("展开线程后的子邮件也带标签胶囊", async () => {
    vi.mocked(api.listInboxThreads).mockResolvedValue({
      items: [
        {
          accountId: 1,
          threadKey: "t1",
          messageCount: 1,
          unreadCount: 1,
          latest: labelledMessage({ labelId: 3, labelName: "通知", labelColor: "teal" }),
        },
      ],
      total: 1,
      offset: 0,
      limit: 200,
    });
    vi.mocked(api.listThreadMessages).mockResolvedValue([
      labelledMessage({ id: 11, uid: 11, labelId: 2, labelName: "账单", labelColor: "orange" }),
    ]);
    render(<InboxPanel />);
    fireEvent.click(await screen.findByLabelText("按会话聚合"));

    const row = await screen.findByRole("button", { name: /张三/ });
    fireEvent.click(row);

    await waitFor(() => {
      const chips = document.querySelectorAll('[data-label-color="orange"]');
      expect(chips.length).toBeGreaterThan(0);
    });
  });
});
describe("收件箱右键立即分类", () => {
  function targetMessage(overrides: Partial<InboxMessage> = {}): InboxMessage {
    return {
      id: 42,
      accountId: 1,
      folderId: 10,
      uid: 42,
      threadKey: "t42",
      subject: "账单邮件",
      fromName: "张三",
      fromAddr: "z@example.com",
      dateUtc: "2026-10-04T02:00:00Z",
      size: 1000,
      hasAttachments: false,
      isRead: false,
      isFlagged: false,
      snippet: "",
      accountEmail: "a@example.com",
      accountName: "测试账号",
      accountColor: "#3366ff",
      folderPath: "INBOX",
      labelId: null,
      labelName: null,
      labelColor: null,
      ...overrides,
    };
  }

  function prepare(settings: Partial<AppSettings> = {}) {
    vi.mocked(api.getAppSettings).mockResolvedValue({ ...SETTINGS, ...settings });
    vi.mocked(api.listInboxMessages).mockResolvedValue({
      items: [targetMessage()],
      total: 1,
      offset: 0,
      limit: 200,
    });
    vi.mocked(api.previewClassifyMessages).mockResolvedValue({
      providerId: 1,
      providerLabel: "jev",
      host: "api.typesafe.ai",
      model: "jev-latest",
      local: false,
      authorizationToken: "tok-1",
      expiresInSeconds: 300,
      targets: [{ messageId: 42, contentHash: "hash-1" }],
    });
    vi.mocked(api.classifyMessages).mockResolvedValue([
      { id: 2, name: "账单", color: "orange", sortOrder: 20, isFallback: false },
    ]);
    return render(<InboxPanel />);
  }

  it("右键出「立即分类」，确认后才外发，且只传点中的那一封", async () => {
    prepare();
    fireEvent.contextMenu(await screen.findByText("账单邮件"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "立即分类" }));

    const dialog = await screen.findByRole("dialog", { name: "确认立即分类" });
    expect(within(dialog).getByText("jev")).toBeTruthy();
    expect(within(dialog).getByText("api.typesafe.ai")).toBeTruthy();
    expect(within(dialog).getByText("jev-latest")).toBeTruthy();
    expect(vi.mocked(api.previewClassifyMessages)).toHaveBeenCalledWith([42]);
    expect(vi.mocked(api.classifyMessages)).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "确认分类" }));
    await waitFor(() => expect(vi.mocked(api.classifyMessages)).toHaveBeenCalledWith("tok-1"));
    expect(vi.mocked(api.classifyMessages)).toHaveBeenCalledTimes(1);
  });

  it("取消确认框不会外发", async () => {
    prepare();
    fireEvent.contextMenu(await screen.findByText("账单邮件"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "立即分类" }));
    const dialog = await screen.findByRole("dialog", { name: "确认立即分类" });
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "确认立即分类" })).toBeNull());
    expect(vi.mocked(api.classifyMessages)).not.toHaveBeenCalled();
  });

  it("开了「下次不要提醒」后右键分类直接外发，不再弹确认框", async () => {
    prepare({ classifySkipManualConfirm: true });
    fireEvent.contextMenu(await screen.findByText("账单邮件"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "立即分类" }));
    await waitFor(() => expect(vi.mocked(api.classifyMessages)).toHaveBeenCalledWith("tok-1"));
    expect(screen.queryByRole("dialog", { name: "确认立即分类" })).toBeNull();
  });
});