//! 文件夹增删改与删单封邮件的前端回归：右键入口、弹窗、二次确认。
//!
//! 虚拟滚动在 jsdom 里拿不到真实尺寸，这里把 `@tanstack/react-virtual` 换成
//! 「全部渲染」的桩，方便断言侧栏文件夹项和邮件行。

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
    listAccounts: vi.fn(),
    listAiProviders: vi.fn(),
    syncStatus: vi.fn(),
    startSync: vi.fn(),
    stopSync: vi.fn(),
    setMessageRead: vi.fn(),
    setMessageFlagged: vi.fn(),
    listLabelGroups: vi.fn().mockResolvedValue([]),
    setMessageGroupValue: vi.fn(),
    clearMessageGroupValue: vi.fn(),
    getMessageBody: vi.fn(),
    getAppSettings: vi.fn(),
    previewClassifyMessages: vi.fn(),
    classifyMessages: vi.fn(),
    setClassifySkipManualConfirm: vi.fn(),
    previewDeleteMessage: vi.fn(),
    deleteMessage: vi.fn(),
    createMailFolder: vi.fn(),
    renameMailFolder: vi.fn(),
    deleteMailFolder: vi.fn(),
    searchMessages: vi.fn(),
    getInboxMessage: vi.fn(),
    listThreadMessages: vi.fn(),
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
  notifyNewMailWhen: "unfocused",
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
    fullPath: "工作",
    kind: "custom",
    messageCount: 0,
    unreadCount: 0,
    delimiter: "/",
    changeable: true,
    allowsChildren: false,
    childCreateBlocked: false,
    ...overrides,
  };
}

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
    folderPath: "工作",
    groupResults: [],
    ...overrides,
  } as InboxMessage;
}

const MESSAGE = message();

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
        messageCount: 0,
        unreadCount: 0,
      },
    ],
    totalUnread: 0,
    totalMessages: 0,
  });
  vi.mocked(api.listAccounts).mockResolvedValue([ACCOUNT]);
  vi.mocked(api.listAiProviders).mockResolvedValue([]);
  vi.mocked(api.getAppSettings).mockResolvedValue(SETTINGS);
  vi.mocked(api.syncStatus).mockResolvedValue([]);
  vi.mocked(api.listInboxMessages).mockResolvedValue({
    items: [MESSAGE],
    total: 1,
    offset: 0,
    limit: 200,
  });
  vi.mocked(api.listInboxThreads).mockResolvedValue({
    items: [],
    total: 0,
    offset: 0,
    limit: 200,
  });
});

afterEach(cleanup);

/** 渲染面板并展开账号，让文件夹项露出来。 */
async function openAccount(): Promise<void> {
  render(<InboxPanel />);
  const accountButton = await screen.findByRole("button", { name: /测试账号/ });
  fireEvent.click(accountButton);
  await screen.findByText("红旗邮件");
}

/** 账号下那条「收件箱」文件夹按钮；统一入口也叫「收件箱」，这里按文件夹类名挑。 */
function accountInboxFolder(): HTMLElement {
  const button = Array.from(document.querySelectorAll<HTMLElement>(".sidebar-folder")).find(
    (element) => element.textContent?.startsWith("收件箱"),
  );
  if (!button) throw new Error("找不到账号下的收件箱文件夹");
  return button;
}

describe("文件夹右键菜单", () => {
  it("服务器不让分层的系统文件夹只给一条禁用说明", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 1, fullPath: "INBOX", kind: "inbox", changeable: false, allowsChildren: false }),
    ]);
    await openAccount();

    fireEvent.contextMenu(accountInboxFolder());
    const menu = await screen.findByRole("menu", { name: "文件夹菜单" });

    expect(within(menu).queryByRole("menuitem", { name: "重命名" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: "删除文件夹" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: "新建子文件夹" })).toBeNull();
    const disabled = within(menu).getByRole("menuitem", { name: "系统文件夹不支持改名或删除" });
    expect((disabled as HTMLButtonElement).disabled).toBe(true);
  });

  it("服务器允许分层的系统文件夹照样给新建子文件夹，只是不给改名删除", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 1, fullPath: "INBOX", kind: "inbox", changeable: false, allowsChildren: true }),
    ]);
    await openAccount();

    fireEvent.contextMenu(accountInboxFolder());
    const menu = await screen.findByRole("menu", { name: "文件夹菜单" });

    expect(within(menu).getByRole("menuitem", { name: "新建子文件夹" })).toBeTruthy();
    expect(within(menu).queryByRole("menuitem", { name: "重命名" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: "删除文件夹" })).toBeNull();
    // 系统文件夹的改名删除还是不开放，说明照旧给。
    const disabled = within(menu).getByRole("menuitem", { name: "系统文件夹不支持改名或删除" });
    expect((disabled as HTMLButtonElement).disabled).toBe(true);
  });

  it("可改但没有层级的文件夹给改名和删除，不给新建子文件夹", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 10, fullPath: "工作", changeable: true, allowsChildren: false }),
    ]);
    await openAccount();

    fireEvent.contextMenu(screen.getByText("工作"));
    const menu = await screen.findByRole("menu", { name: "文件夹菜单" });

    expect(within(menu).getByRole("menuitem", { name: "重命名" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "删除文件夹" })).toBeTruthy();
    expect(within(menu).queryByRole("menuitem", { name: "新建子文件夹" })).toBeNull();
  });

  it("服务器支持层级时才多一条新建子文件夹", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 10, fullPath: "工作", changeable: true, allowsChildren: true }),
    ]);
    await openAccount();

    fireEvent.contextMenu(screen.getByText("工作"));
    const menu = await screen.findByRole("menu", { name: "文件夹菜单" });

    expect(within(menu).getByRole("menuitem", { name: "新建子文件夹" })).toBeTruthy();
  });

  it("服务商拒绝过后只给一条置灰说明，不再开放新建子文件夹", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({
        folderId: 10,
        fullPath: "订阅邮件",
        changeable: false,
        allowsChildren: false,
        childCreateBlocked: true,
      }),
    ]);
    await openAccount();

    fireEvent.contextMenu(screen.getByText("订阅邮件"));
    const menu = await screen.findByRole("menu", { name: "文件夹菜单" });

    expect(within(menu).queryByRole("menuitem", { name: "新建子文件夹" })).toBeNull();
    const hint = within(menu).getByRole("menuitem", {
      name: "服务商不允许在此文件夹下建子文件夹",
    });
    expect((hint as HTMLButtonElement).disabled).toBe(true);
  });
  it("点重命名弹窗预填末段名字，保存后调改名接口", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 10, fullPath: "工作/项目", changeable: true, allowsChildren: false }),
    ]);
    vi.mocked(api.renameMailFolder).mockResolvedValue(undefined);
    await openAccount();

    fireEvent.contextMenu(screen.getByText("工作/项目"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "重命名" }));

    const dialog = await screen.findByRole("dialog", { name: "文件夹" });
    const input = within(dialog).getByRole("textbox") as HTMLInputElement;
    expect(input.value).toBe("项目");
    fireEvent.change(input, { target: { value: "新项目" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(vi.mocked(api.renameMailFolder)).toHaveBeenCalledWith(10, "新项目"),
    );
  });

  it("点删除文件夹弹确认框，确认后调删除接口", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 10, fullPath: "工作", changeable: true, allowsChildren: false }),
    ]);
    vi.mocked(api.deleteMailFolder).mockResolvedValue(undefined);
    await openAccount();

    fireEvent.contextMenu(screen.getByText("工作"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "删除文件夹" }));

    const dialog = await screen.findByRole("dialog", { name: "确认删除文件夹" });
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));

    await waitFor(() => expect(vi.mocked(api.deleteMailFolder)).toHaveBeenCalledWith(10));
  });
});

describe("账号右键新建文件夹", () => {
  it("右键账号给新建文件夹入口，保存后调新建接口", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([folder()]);
    vi.mocked(api.createMailFolder).mockResolvedValue(undefined);
    render(<InboxPanel />);
    const accountButton = await screen.findByRole("button", { name: /测试账号/ });

    fireEvent.contextMenu(accountButton);
    fireEvent.click(await screen.findByRole("menuitem", { name: "新建文件夹" }));

    const dialog = await screen.findByRole("dialog", { name: "文件夹" });
    fireEvent.change(within(dialog).getByRole("textbox"), { target: { value: "归档" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(vi.mocked(api.createMailFolder)).toHaveBeenCalledWith(1, undefined, "归档"),
    );
  });

  it("右键文件夹新建子文件夹时带上父文件夹编号", async () => {
    vi.mocked(api.listInboxFolders).mockResolvedValue([
      folder({ folderId: 10, fullPath: "工作", changeable: true, allowsChildren: true }),
    ]);
    vi.mocked(api.createMailFolder).mockResolvedValue(undefined);
    await openAccount();

    fireEvent.contextMenu(screen.getByText("工作"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "新建子文件夹" }));

    const dialog = await screen.findByRole("dialog", { name: "文件夹" });
    fireEvent.change(within(dialog).getByRole("textbox"), { target: { value: "子项目" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(vi.mocked(api.createMailFolder)).toHaveBeenCalledWith(1, 10, "子项目"),
    );
  });
  it("建子文件夹被服务商拒绝后刷新侧栏收口入口", async () => {
    vi.mocked(api.listInboxFolders)
      .mockResolvedValueOnce([
        folder({
          folderId: 10,
          fullPath: "订阅邮件",
          changeable: false,
          allowsChildren: true,
        }),
      ])
      .mockResolvedValue([
        folder({
          folderId: 10,
          fullPath: "订阅邮件",
          changeable: false,
          allowsChildren: false,
          childCreateBlocked: true,
        }),
      ]);
    vi.mocked(api.createMailFolder).mockRejectedValue(
      new Error("服务商不允许在这个文件夹下面建子文件夹"),
    );
    await openAccount();

    fireEvent.contextMenu(screen.getByText("订阅邮件"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "新建子文件夹" }));

    const dialog = await screen.findByRole("dialog", { name: "文件夹" });
    fireEvent.change(within(dialog).getByRole("textbox"), { target: { value: "子项目" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(vi.mocked(api.listInboxFolders).mock.calls.length).toBeGreaterThanOrEqual(2),
    );
  });
});

describe("删除单封邮件", () => {
  it("普通文件夹直接移到已删除，不弹二次确认", async () => {
    vi.mocked(api.previewDeleteMessage).mockResolvedValue({ targetFolder: "已删除", permanent: false });
    vi.mocked(api.deleteMessage).mockResolvedValue(undefined);
    render(<InboxPanel />);
    await screen.findByText("主题 1");

    fireEvent.contextMenu(screen.getByText("主题 1"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "删除" }));

    await waitFor(() => expect(vi.mocked(api.deleteMessage)).toHaveBeenCalledWith(1, false));
    expect(screen.queryByRole("dialog", { name: "确认彻底删除邮件" })).toBeNull();
  });

  it("已在已删除里要求二次确认，点彻底删除才真删", async () => {
    vi.mocked(api.previewDeleteMessage).mockResolvedValue({ targetFolder: null, permanent: true });
    vi.mocked(api.deleteMessage).mockResolvedValue(undefined);
    render(<InboxPanel />);
    await screen.findByText("主题 1");

    fireEvent.contextMenu(screen.getByText("主题 1"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "删除" }));

    const dialog = await screen.findByRole("dialog", { name: "确认彻底删除邮件" });
    expect(vi.mocked(api.deleteMessage)).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "彻底删除" }));
    await waitFor(() => expect(vi.mocked(api.deleteMessage)).toHaveBeenCalledWith(1, true));
  });

  it("二次确认里点取消不会删任何东西", async () => {
    vi.mocked(api.previewDeleteMessage).mockResolvedValue({ targetFolder: null, permanent: true });
    vi.mocked(api.deleteMessage).mockResolvedValue(undefined);
    render(<InboxPanel />);
    await screen.findByText("主题 1");

    fireEvent.contextMenu(screen.getByText("主题 1"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "删除" }));

    const dialog = await screen.findByRole("dialog", { name: "确认彻底删除邮件" });
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "确认彻底删除邮件" })).toBeNull(),
    );
    expect(vi.mocked(api.deleteMessage)).not.toHaveBeenCalled();
  });
});