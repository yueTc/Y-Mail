//! 「红旗邮件」左侧入口回归：每个账号的收件箱下面多一条，点了按账号看标红邮件。

import { act, cleanup, createEvent, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import InboxPanel, { SEARCH_PLACEHOLDER } from "../InboxPanel";
import type {
  Account,
  AppSettings,
  GroupResult,
  InboxFolder,
  InboxMessage,
  LabelGroup,
} from "../api";
import { api } from "../api";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("../ComposePanel", () => ({
  default: ({ request }: { request: { kind: string; sourceMessageId?: number } }) => (
    <div role="region" aria-label="写信窗格">
      {request.kind}:{request.sourceMessageId ?? ""}
    </div>
  ),
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
    deleteAccount: vi.fn(),
    listProxies: vi.fn(),
    listInboxMessages: vi.fn(),
    listInboxFolders: vi.fn(),
    inboxSummary: vi.fn(),
    listAiProviders: vi.fn(),
    previewClassifyMessages: vi.fn(),
    classifyMessages: vi.fn(),
    getAppSettings: vi.fn(),
    setClassifySkipManualConfirm: vi.fn(),
    listLabelGroups: vi.fn(),
    setMessageGroupValue: vi.fn(),
    clearMessageGroupValue: vi.fn(),
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
  vi.mocked(api.stopSync).mockResolvedValue(undefined);
  vi.mocked(api.deleteAccount).mockResolvedValue(undefined);
  vi.mocked(api.listLabelGroups).mockResolvedValue([]);
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
    expect((screen.getByRole("button", { name: "按会话聚合" }) as HTMLButtonElement).disabled).toBe(true);
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
  it("按会话聚合用线程列表图标在两种状态间切换", async () => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });
    const groupingButton = screen.getByRole("button", { name: "按会话聚合" });
    expect(groupingButton.getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(groupingButton);
    expect(groupingButton.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(groupingButton);
    expect(groupingButton.getAttribute("aria-pressed")).toBe("false");
  });

  it("收件箱下面排着未读 / 红旗 / 草稿 / 已发送", async () => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });
    const labels = Array.from(document.querySelectorAll(".inbox-sidebar .sidebar-item")).map(
      (element) => (element.textContent ?? "").replace(/\d+$/, "").trim(),
    );
    expect(labels).toEqual([
      "收件箱",
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
    fireEvent.click(screen.getByRole("button", { name: "按会话聚合" }));
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

    const unreadButton = screen.getByRole("button", { name: "只看未读" });
    expect(unreadButton.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(unreadButton);
    expect(unreadButton.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(unreadButton);

    // 先手动打开会话聚合，验证未读筛选在聚合模式下走线程接口。
    fireEvent.click(screen.getByRole("button", { name: "按会话聚合" }));
    fireEvent.click(screen.getByRole("button", { name: /所有未读/ }));
    await waitFor(() =>
      expect(api.listInboxThreads).toHaveBeenCalledWith(
        expect.objectContaining({ unreadOnly: true }),
      ),
    );
    const lockedUnreadButton = screen.getByRole("button", { name: "只看未读" }) as HTMLButtonElement;
    expect(lockedUnreadButton.getAttribute("aria-pressed")).toBe("true");
    expect(lockedUnreadButton.disabled).toBe(true);
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
    fireEvent.click(screen.getByRole("button", { name: "按会话聚合" }));

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
describe("邮箱栏右键添加和删除", () => {
  async function openDelete() {
    fireEvent.contextMenu(await screen.findByRole("button", { name: /测试账号/ }));
    const menu = screen.getByRole("menu", { name: "邮箱菜单" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "删除邮箱" }));
    return screen.getByRole("dialog", { name: "确认删除邮箱" });
  }

  it("空白处右键可以添加邮箱，原有添加按钮继续保留", async () => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });
    expect(screen.getByRole("button", { name: "＋ 添加邮箱" })).toBeTruthy();
    fireEvent.contextMenu(screen.getByRole("complementary", { name: "邮箱栏" }));
    const menu = screen.getByRole("menu", { name: "邮箱菜单" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["添加邮箱"]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "添加邮箱" }));
    expect(await screen.findByRole("dialog", { name: "添加邮箱" })).toBeTruthy();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("统一入口不触发空白菜单，邮箱右键保留原有操作", async () => {
    render(<InboxPanel />);
    const account = await screen.findByRole("button", { name: /测试账号/ });
    expect(fireEvent.contextMenu(screen.getByRole("button", { name: /收件箱/ }))).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
    fireEvent.contextMenu(account);
    const menu = screen.getByRole("menu", { name: "邮箱菜单" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "编辑", "新建文件夹", "删除邮箱",
    ]);
    expect(within(menu).queryByRole("menuitem", { name: "添加邮箱" })).toBeNull();
  });

  it.each(["点击外部", "按 Esc", "滚动"])("空白处菜单在%s时关闭", async (action) => {
    render(<InboxPanel />);
    await screen.findByRole("button", { name: /测试账号/ });
    const sidebar = screen.getByRole("complementary", { name: "邮箱栏" });
    fireEvent.contextMenu(sidebar);
    expect(screen.getByRole("menu")).toBeTruthy();
    if (action === "点击外部") fireEvent.click(document.body);
    if (action === "按 Esc") fireEvent.keyDown(document, { key: "Escape" });
    if (action === "滚动") fireEvent.scroll(sidebar);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("删除先说明范围，取消或按 Esc 都不停止同步、不删除", async () => {
    render(<InboxPanel />);
    const dialog = await openDelete();
    expect(within(dialog).getByText("将移除本机账号、关联邮件数据和保存的凭据，不会删除服务器上的邮件。")).toBeTruthy();
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "取消" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull();
    const reopened = await openDelete();
    fireEvent.keyDown(reopened, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull();
    expect(api.deleteAccount).not.toHaveBeenCalled();
    expect(api.stopSync).not.toHaveBeenCalled();
  });

  it("删除当前邮箱时先停止该账号同步，成功后刷新并返回收件箱", async () => {
    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /测试账号/ }));
    await waitFor(() => expect(api.listInboxMessages).toHaveBeenLastCalledWith(expect.objectContaining({ accountId: 1 })));
    const dialog = await openDelete();
    vi.mocked(api.inboxSummary).mockResolvedValueOnce({ accounts: [], totalUnread: 0, totalMessages: 0 });
    vi.mocked(api.listInboxFolders).mockResolvedValueOnce([]);
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(api.deleteAccount).toHaveBeenCalledWith(1));
    expect(api.stopSync).toHaveBeenCalledWith(1);
    expect(vi.mocked(api.stopSync).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.deleteAccount).mock.invocationCallOrder[0]);
    await waitFor(() => expect(screen.queryByRole("button", { name: /测试账号/ })).toBeNull());
    expect(screen.getByRole("button", { name: "收件箱" }).classList.contains("active")).toBe(true);
    await waitFor(() => {
      const query = vi.mocked(api.listInboxMessages).mock.calls.at(-1)?.[0];
      expect(query).toBeTruthy();
      expect(query?.accountId).toBeUndefined();
      expect(query?.folderId).toBeUndefined();
    });
    expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull();
  });

  it("统一视图中删除邮箱也会重新读取邮件列表", async () => {
    render(<InboxPanel />);
    const dialog = await openDelete();
    const previousLoads = vi.mocked(api.listInboxMessages).mock.calls.length;
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull());
    await waitFor(() => expect(vi.mocked(api.listInboxMessages).mock.calls.length).toBeGreaterThan(previousLoads));
    expect(screen.getByRole("button", { name: /收件箱/ }).classList.contains("active")).toBe(true);
  });

  it("删除其他邮箱时继续停留在当前邮箱", async () => {
    const first = { accountId: 1, email: "a@example.com", displayName: "测试账号", color: "blue", enabled: true, messageCount: 3, unreadCount: 2 };
    const second = { ...first, accountId: 2, email: "b@example.com", displayName: "第二邮箱" };
    vi.mocked(api.inboxSummary).mockResolvedValue({ accounts: [first, second], totalMessages: 6, totalUnread: 4 });
    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /第二邮箱/ }));
    await waitFor(() => expect(api.listInboxMessages).toHaveBeenLastCalledWith(expect.objectContaining({ accountId: 2 })));
    const dialog = await openDelete();
    vi.mocked(api.inboxSummary).mockResolvedValue({ accounts: [second], totalMessages: 3, totalUnread: 2 });
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull());
    expect(screen.getByRole("button", { name: /第二邮箱/ }).classList.contains("active")).toBe(true);
    await waitFor(() => expect(api.listInboxMessages).toHaveBeenLastCalledWith(expect.objectContaining({ accountId: 2 })));
  });

  it("下一次状态轮询也不会把已删除邮箱的数量算回来", async () => {
    const intervalSpy = vi.spyOn(window, "setInterval");
    vi.mocked(api.syncStatus).mockResolvedValue([{
      accountId: 1, email: "a@example.com", state: "idle", stateLabel: "实时等待",
      progress: 3, total: 3, message: "", needsReauth: false, updatedAt: "",
    }]);
    render(<InboxPanel />);
    const dialog = await openDelete();
    expect(await screen.findByText("实时等待 3/3")).toBeTruthy();
    vi.mocked(api.inboxSummary).mockResolvedValue({ accounts: [], totalMessages: 0, totalUnread: 0 });
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: /测试账号/ })).toBeNull());
    const poll = intervalSpy.mock.calls.find((call) => call[1] === 2000)?.[0];
    expect(typeof poll).toBe("function");
    await act(async () => { if (typeof poll === "function") await poll(); });
    expect(screen.queryByText("实时等待 3/3")).toBeNull();
    intervalSpy.mockRestore();
  });

  it("从空白菜单切到文件夹右键时，只留下文件夹菜单", async () => {
    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /测试账号/ }));
    fireEvent.contextMenu(screen.getByRole("complementary", { name: "邮箱栏" }));
    expect(screen.getByRole("menu", { name: "邮箱菜单" })).toBeTruthy();
    const inboxFolder = Array.from(document.querySelectorAll<HTMLElement>(".sidebar-folder")).find(
      (element) => element.textContent?.startsWith("收件箱"),
    );
    expect(inboxFolder).toBeTruthy();
    fireEvent.contextMenu(inboxFolder!);
    expect(screen.queryByRole("menu", { name: "邮箱菜单" })).toBeNull();
    expect(screen.getByRole("menu", { name: "文件夹菜单" })).toBeTruthy();
  });

  it("删除失败保留确认框和账号，可以重试", async () => {
    vi.mocked(api.deleteAccount).mockRejectedValueOnce(new Error("本机数据暂时不可写"));
    render(<InboxPanel />);
    const dialog = await openDelete();
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    expect(await within(dialog).findByRole("alert")).toHaveProperty("textContent", "操作失败：本机数据暂时不可写");
    expect(screen.getByRole("button", { name: /测试账号/ })).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull());
    expect(api.deleteAccount).toHaveBeenCalledTimes(2);
  });

  it("停止同步失败时不调用删除", async () => {
    vi.mocked(api.stopSync).mockRejectedValueOnce(new Error("无法停止同步"));
    render(<InboxPanel />);
    const dialog = await openDelete();
    fireEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    expect(await within(dialog).findByRole("alert")).toHaveProperty("textContent", "操作失败：无法停止同步");
    expect(api.deleteAccount).not.toHaveBeenCalled();
  });

  it("提交中锁住确认和取消，避免重复删除", async () => {
    let completeDelete: () => void = () => {};
    vi.mocked(api.deleteAccount).mockReturnValueOnce(new Promise<void>((resolve) => { completeDelete = resolve; }));
    render(<InboxPanel />);
    const dialog = await openDelete();
    const confirm = within(dialog).getByRole("button", { name: "确认删除" });
    fireEvent.click(confirm);
    await waitFor(() => expect(api.deleteAccount).toHaveBeenCalledTimes(1));
    expect(confirm).toHaveProperty("disabled", true);
    expect(within(dialog).getByRole("button", { name: "取消" })).toHaveProperty("disabled", true);
    fireEvent.click(confirm);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "确认删除邮箱" })).toBeTruthy();
    expect(api.deleteAccount).toHaveBeenCalledTimes(1);
    completeDelete();
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "确认删除邮箱" })).toBeNull());
  });
});

describe("列表标签胶囊", () => {
  const billPill: GroupResult = {
    groupId: 1,
    groupName: "内容分类",
    kind: "choice",
    labelId: 2,
    labelName: "账单",
    labelColor: "orange",
    scoreMin: null,
    scoreMax: null,
    scoreValue: null,
  };
  const notifyPill: GroupResult = {
    groupId: 1,
    groupName: "内容分类",
    kind: "choice",
    labelId: 3,
    labelName: "通知",
    labelColor: "teal",
    scoreMin: null,
    scoreMax: null,
    scoreValue: null,
  };
  const urgentPill: GroupResult = {
    groupId: 3,
    groupName: "紧急程度",
    kind: "score",
    labelId: null,
    labelName: null,
    labelColor: null,
    scoreMin: 1,
    scoreMax: 5,
    scoreValue: 5,
  };

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
      groupResults: [billPill],
      ...overrides,
    };
  }

  it("平铺行把胶囊放在时间前面，点一下按 tag 搜索", async () => {
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
      expect(searchBox.value).toBe("tag:内容分类:账单");
    });
    await waitFor(() =>
      expect(vi.mocked(api.searchMessages)).toHaveBeenCalledWith(
        expect.objectContaining({ raw: "tag:内容分类:账单" }),
      ),
    );
  });

  it("没有标签时不渲染胶囊", async () => {
    vi.mocked(api.listInboxMessages).mockResolvedValue({
      items: [labelledMessage({ groupResults: [] })],
      total: 1,
      offset: 0,
      limit: 200,
    });
    render(<InboxPanel />);
    await screen.findByText("账单邮件");
    expect(screen.queryByRole("button", { name: "账单" })).toBeNull();
  });

  it.each([
    { fromName: "huggingface", count: 1, unread: 0, expected: "huggingface" },
    { fromName: "很长的发件方名称".repeat(20), count: 1234, unread: 2, expected: "很长的发件方名称".repeat(20) },
    { fromName: "   ", count: 2, unread: 1, expected: "z@example.com" },
  ])("会话标题只显示发件方和总封数：$count 封", async ({ fromName, count, unread, expected }) => {
    vi.mocked(api.listInboxThreads).mockResolvedValue({
      items: [{
        accountId: 1,
        threadKey: "t1",
        messageCount: count,
        unreadCount: unread,
        latest: labelledMessage({ fromName, groupResults: [notifyPill], isFlagged: true, hasAttachments: true }),
      }],
      total: 1,
      offset: 0,
      limit: 200,
    });
    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "按会话聚合" }));

    const row = await screen.findByRole("button", { name: `${expected} 共 ${count} 封` });
    expect(row.getAttribute("aria-expanded")).toBe("false");
    expect(row.getAttribute("data-read")).toBe(unread > 0 ? "false" : "true");
    expect(within(row).getByTitle(expected).textContent).toBe(expected);
    expect(within(row).queryByText("账单邮件")).toBeNull();
    expect(within(row).queryByRole("button")).toBeNull();
    expect(within(row).queryByTitle("有附件")).toBeNull();
    expect(row.querySelector(".inbox-row-time")).toBeNull();
    expect(row.querySelector(".dot")).toBeNull();
    expect(api.listThreadMessages).not.toHaveBeenCalled();
  });

  it("展开线程后的子邮件也带标签胶囊", async () => {
    vi.mocked(api.listInboxThreads).mockResolvedValue({
      items: [
        {
          accountId: 1,
          threadKey: "t1",
          messageCount: 1,
          unreadCount: 1,
          latest: labelledMessage({ groupResults: [notifyPill] }),
        },
      ],
      total: 1,
      offset: 0,
      limit: 200,
    });
    vi.mocked(api.listThreadMessages).mockResolvedValue([
      labelledMessage({ id: 11, uid: 11 }),
    ]);
    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "按会话聚合" }));

    const row = await screen.findByRole("button", { name: "张三 共 1 封" });
    fireEvent.click(row);

    await waitFor(() => {
      const chips = document.querySelectorAll('[data-label-color="orange"]');
      expect(chips.length).toBeGreaterThan(0);
    });
  });

  it("平铺行把每组的胶囊都排在时间前面，顺序按组", async () => {
    vi.mocked(api.listInboxMessages).mockResolvedValue({
      items: [labelledMessage({ groupResults: [billPill, urgentPill] })],
      total: 1,
      offset: 0,
      limit: 200,
    });
    render(<InboxPanel />);

    const bill = await screen.findByRole("button", { name: "账单" });
    const urgent = screen.getByRole("button", { name: "5" });
    expect(urgent.getAttribute("data-score-band")).toBe("5");
    const time = document.querySelector(".inbox-row-time") as HTMLElement;
    expect(bill.compareDocumentPosition(time) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(urgent.compareDocumentPosition(time) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(bill.compareDocumentPosition(urgent) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("分组标题支持键盘展开收起，子邮件保留标签和星标操作", async () => {
    const child = labelledMessage({ id: 11, uid: 11, groupResults: [billPill, urgentPill], hasAttachments: true });
    vi.mocked(api.listInboxThreads).mockResolvedValue({
      items: [{
        accountId: 1,
        threadKey: "t1",
        messageCount: 2,
        unreadCount: 1,
        latest: child,
      }],
      total: 1,
      offset: 0,
      limit: 200,
    });
    vi.mocked(api.listThreadMessages).mockResolvedValue([child]);
    vi.mocked(api.setMessageFlagged).mockResolvedValue({ changed: true, synced: true });
    render(<InboxPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "按会话聚合" }));

    const row = await screen.findByRole("button", { name: "张三 共 2 封" });
    row.focus();
    fireEvent.keyDown(row, { key: "Enter" });
    expect(row.getAttribute("aria-expanded")).toBe("true");
    const flag = await screen.findByRole("button", { name: "标红" });
    expect(await screen.findByRole("button", { name: "账单" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "5" })).toBeTruthy();
    expect(screen.getByTitle("有附件")).toBeTruthy();
    expect(screen.getByText("来自 测试账号")).toBeTruthy();

    fireEvent.click(flag);
    await waitFor(() => expect(api.setMessageFlagged).toHaveBeenCalledWith(11, true));
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(api.getMessageBody).not.toHaveBeenCalled();
    fireEvent.keyDown(row, { key: " " });
    expect(row.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("button", { name: "账单" })).toBeNull();
    expect(document.activeElement).toBe(row);
    fireEvent.click(row);
    expect(await screen.findByRole("button", { name: "账单" })).toBeTruthy();
    expect(api.listThreadMessages).toHaveBeenCalledTimes(1);
    expect(api.listThreadMessages).toHaveBeenCalledWith(1, "t1", 200);
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
      groupResults: [],
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
      enabledGroups: [{ name: "内容分类", kind: "choice" }],
    });
    vi.mocked(api.classifyMessages).mockResolvedValue([
      { messageId: 42, groupResults: [] },
    ]);
    return render(<InboxPanel />);
  }

  it("邮件右键菜单提供回复和转发，并针对点中的邮件打开写信窗格", async () => {
    prepare();
    const toolbar = document.querySelector(".inbox-toolbar");
    expect(toolbar).toBeTruthy();
    expect(within(toolbar as HTMLElement).queryByRole("button", { name: "回复" })).toBeNull();
    expect(within(toolbar as HTMLElement).queryByRole("button", { name: "转发" })).toBeNull();

    const subject = await screen.findByText("账单邮件");
    fireEvent.contextMenu(subject);
    const menu = await screen.findByRole("menu", { name: "邮件菜单" });
    expect(within(menu).getByRole("menuitem", { name: "回复" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "转发" })).toBeTruthy();

    fireEvent.click(within(menu).getByRole("menuitem", { name: "回复" }));
    expect((await screen.findByRole("region", { name: "写信窗格" })).textContent).toBe("reply:42");

    fireEvent.contextMenu(subject);
    const forwardMenu = await screen.findByRole("menu", { name: "邮件菜单" });
    fireEvent.click(within(forwardMenu).getByRole("menuitem", { name: "转发" }));
    expect((await screen.findByRole("region", { name: "写信窗格" })).textContent).toBe("forward:42");
  });

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

describe("收件箱右键按组修改", () => {
  const GROUPS: LabelGroup[] = [
    {
      id: 1,
      name: "内容分类",
      kind: "choice",
      enabled: true,
      confidenceThreshold: 0.75,
      scoreMin: null,
      scoreMax: null,
      sortOrder: 10,
      labels: [
        { id: 1, groupId: 1, name: "客户", color: "blue", sortOrder: 10, isFallback: false },
        { id: 2, groupId: 1, name: "账单", color: "orange", sortOrder: 20, isFallback: false },
        { id: 3, groupId: 1, name: "通知", color: "teal", sortOrder: 30, isFallback: false },
      ],
    },
    {
      id: 2,
      name: "需要我回复",
      kind: "boolean",
      enabled: true,
      confidenceThreshold: 0.75,
      scoreMin: null,
      scoreMax: null,
      sortOrder: 20,
      labels: [
        { id: 4, groupId: 2, name: "需要回复", color: "red", sortOrder: 10, isFallback: false },
        { id: 5, groupId: 2, name: "不用回复", color: "gray", sortOrder: 20, isFallback: false },
      ],
    },
    {
      id: 3,
      name: "紧急程度",
      kind: "score",
      enabled: true,
      confidenceThreshold: 0.75,
      scoreMin: 1,
      scoreMax: 5,
      sortOrder: 30,
      labels: [],
    },
  ];

  function menuMessage(overrides: Partial<InboxMessage> = {}): InboxMessage {
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
      groupResults: [],
      ...overrides,
    };
  }

  function prepareMenu() {
    vi.mocked(api.listLabelGroups).mockResolvedValue(GROUPS);
    vi.mocked(api.listInboxMessages).mockResolvedValue({
      items: [menuMessage()],
      total: 1,
      offset: 0,
      limit: 200,
    });
    render(<InboxPanel />);
  }

  async function openManualGroup(name: string) {
    fireEvent.contextMenu(await screen.findByText("账单邮件"));
    const menu = await screen.findByRole("menu", { name: "邮件菜单" });
    fireEvent.mouseEnter(within(menu).getByRole("menuitem", { name: "手动分类" }));
    fireEvent.mouseEnter(await screen.findByRole("menuitem", { name }));
    return screen.getByRole("menu", { name });
  }

  it("右键先显示两种分类入口，悬停手动分类及标签组后才显示可选值", async () => {
    prepareMenu();
    fireEvent.contextMenu(await screen.findByText("账单邮件"));
    const menu = await screen.findByRole("menu", { name: "邮件菜单" });
    expect(within(menu).getByRole("menuitem", { name: "立即分类" })).toBeTruthy();
    expect(within(menu).queryByRole("menuitem", { name: "内容分类" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: "账单" })).toBeNull();

    fireEvent.mouseEnter(within(menu).getByRole("menuitem", { name: "手动分类" }));
    const group = await screen.findByRole("menuitem", { name: "内容分类" });
    expect(screen.queryByRole("menuitem", { name: "客户" })).toBeNull();
    fireEvent.mouseEnter(group);
    expect(screen.getByRole("menuitem", { name: "客户" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "账单" })).toBeTruthy();
    expect(screen.getAllByRole("menuitem", { name: "清除本组结果" })).toHaveLength(1);

    fireEvent.mouseEnter(screen.getByRole("menuitem", { name: "需要我回复" }));
    expect(screen.queryByRole("menuitem", { name: "账单" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "需要回复" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "不用回复" })).toBeTruthy();
    fireEvent.mouseEnter(screen.getByRole("menuitem", { name: "紧急程度" }));
    for (const value of ["1", "2", "3", "4", "5"]) {
      expect(screen.getByRole("menuitem", { name: value })).toBeTruthy();
    }
    expect(api.setMessageGroupValue).not.toHaveBeenCalled();
    expect(api.previewClassifyMessages).not.toHaveBeenCalled();
    expect(api.classifyMessages).not.toHaveBeenCalled();
  });

  it("点击标签才写入本地手动结果，收起菜单并刷新列表", async () => {
    vi.mocked(api.setMessageGroupValue).mockResolvedValue(undefined);
    prepareMenu();
    const menu = await openManualGroup("内容分类");
    const loadsBefore = vi.mocked(api.listInboxMessages).mock.calls.length;
    fireEvent.click(within(menu).getByRole("menuitem", { name: "账单" }));
    await waitFor(() => expect(api.setMessageGroupValue).toHaveBeenCalledWith(42, 1, 2, null));
    await waitFor(() => expect(vi.mocked(api.listInboxMessages).mock.calls.length).toBeGreaterThan(loadsBefore));
    expect(screen.queryByRole("menu", { name: "邮件菜单" })).toBeNull();
    expect(api.classifyMessages).not.toHaveBeenCalled();
  });

  it("点击分数仍写入对应组的数值", async () => {
    vi.mocked(api.setMessageGroupValue).mockResolvedValue(undefined);
    prepareMenu();
    const menu = await openManualGroup("紧急程度");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "4" }));
    await waitFor(() => expect(api.setMessageGroupValue).toHaveBeenCalledWith(42, 3, null, 4));
  });

  it("每个组里的清除入口只清除对应组", async () => {
    vi.mocked(api.clearMessageGroupValue).mockResolvedValue(undefined);
    prepareMenu();
    const menu = await openManualGroup("需要我回复");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "清除本组结果" }));
    await waitFor(() => expect(api.clearMessageGroupValue).toHaveBeenCalledWith(42, 2));
  });

  it("子菜单滚动不关闭整组菜单，按 Esc 或点击外面会全部关闭", async () => {
    prepareMenu();
    const menu = await openManualGroup("内容分类");
    fireEvent.scroll(menu);
    expect(screen.getByRole("menuitem", { name: "账单" })).toBeTruthy();
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "邮件菜单" })).toBeNull();
    await openManualGroup("内容分类");
    fireEvent.click(document.body);
    expect(screen.queryByRole("menu", { name: "邮件菜单" })).toBeNull();
  });

  it("去掉搜索按钮后仍能用 Ctrl+K 聚焦、回车本地搜索和 Esc 退出", async () => {
    vi.mocked(api.searchMessages).mockResolvedValue({
      items: [], total: 0, offset: 0, limit: 200, deepSynced: false, deepError: null,
    });
    render(<InboxPanel />);
    const searchBox = screen.getByLabelText("搜索邮件") as HTMLInputElement;

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(document.activeElement).toBe(searchBox);
    fireEvent.change(searchBox, { target: { value: "账单" } });
    fireEvent.keyDown(searchBox, { key: "Enter" });
    await waitFor(() => expect(api.searchMessages).toHaveBeenCalledWith(
      expect.objectContaining({ raw: "账单", deep: false }),
    ));
    await screen.findByText("没有找到匹配的邮件。");
    expect(screen.queryByText(/共找到 .* 封，当前已显示 .* 封/)).toBeNull();
    expect(screen.queryByRole("button", { name: "继续联网补历史" })).toBeNull();
    expect(screen.queryByRole("button", { name: "退出搜索" })).toBeNull();
    fireEvent.keyDown(searchBox, { key: "Escape" });
    expect(screen.queryByText("共找到 0 封，当前已显示 0 封")).toBeNull();
    expect(searchBox.value).toBe("");
    expect(searchBox.parentElement?.querySelector("kbd")?.textContent).toBe("Ctrl+K");
  });

  it.each([
    { scenario: "正常续载", failFirst: false },
    { scenario: "续载失败后重试", failFirst: true },
  ])("搜索结果滚到列表底部附近继续加载未显示的结果：$scenario", async ({ failFirst }) => {
    const searchMessage = (id: number, subject: string): InboxMessage => ({
      id,
      accountId: 1,
      folderId: 10,
      uid: id,
      threadKey: `search-${id}`,
      subject,
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
      groupResults: [],
    });
    const first = searchMessage(101, "第一封结果");
    const second = searchMessage(102, "第二封结果");
    const remaining = {
      items: [{ message: second, snippet: [] }],
      total: 2,
      offset: 1,
      limit: 200,
      deepSynced: false,
      deepError: null,
    };
    let finish!: (value: typeof remaining) => void;
    let fail!: (reason: Error) => void;
    const pending = new Promise<typeof remaining>((resolve, reject) => {
      finish = resolve;
      fail = reject;
    });
    vi.mocked(api.searchMessages)
      .mockResolvedValueOnce({
        ...remaining,
        items: [{ message: first, snippet: [] }],
        offset: 0,
      })
      .mockReturnValueOnce(pending);
    if (failFirst) vi.mocked(api.searchMessages).mockResolvedValueOnce(remaining);
    render(<InboxPanel />);
    const searchBox = screen.getByLabelText("搜索邮件") as HTMLInputElement;
    fireEvent.change(searchBox, { target: { value: "关键词" } });
    fireEvent.keyDown(searchBox, { key: "Enter" });
    await screen.findByText("第一封结果");

    expect(screen.queryByRole("button", { name: /继续加载/ })).toBeNull();
    expect(api.searchMessages).toHaveBeenCalledTimes(1);
    const scroll = document.querySelector(".inbox-scroll") as HTMLDivElement;
    Object.defineProperties(scroll, {
      scrollHeight: { configurable: true, value: 1000 },
      scrollTop: { configurable: true, writable: true, value: 559 },
      clientHeight: { configurable: true, value: 200 },
    });
    fireEvent.scroll(scroll);
    expect(api.searchMessages).toHaveBeenCalledTimes(1);

    // 未按回车的新输入不能混进当前搜索结果。
    fireEvent.change(searchBox, { target: { value: "尚未提交的新词" } });
    scroll.scrollTop = 560;
    act(() => {
      fireEvent.scroll(scroll);
      fireEvent.scroll(scroll);
    });
    expect(api.searchMessages).toHaveBeenCalledTimes(2);
    expect(api.searchMessages).toHaveBeenLastCalledWith({
      raw: "关键词", offset: 1, limit: 200, deep: false,
    });
    fireEvent.scroll(scroll);
    expect(api.searchMessages).toHaveBeenCalledTimes(2);

    if (failFirst) {
      await act(async () => fail(new Error("续载失败")));
      await screen.findByText(/续载失败/);
      expect(screen.getByText("第一封结果")).toBeTruthy();
      fireEvent.scroll(scroll);
      await waitFor(() => expect(api.searchMessages).toHaveBeenCalledTimes(3));
      expect(api.searchMessages).toHaveBeenLastCalledWith({
        raw: "关键词", offset: 1, limit: 200, deep: false,
      });
    } else {
      await act(async () => finish(remaining));
    }

    await screen.findByText("第二封结果");
    expect(screen.getByText("第一封结果")).toBeTruthy();
    fireEvent.scroll(scroll);
    expect(api.searchMessages).toHaveBeenCalledTimes(failFirst ? 3 : 2);
  });

  it("有内容时用小叉替换快捷键，清空后返回普通列表并聚焦搜索框", async () => {
    vi.mocked(api.searchMessages).mockResolvedValue({
      items: [], total: 0, offset: 0, limit: 200, deepSynced: false, deepError: null,
    });
    prepareMenu();
    await screen.findByText("账单邮件");
    const searchBox = screen.getByLabelText("搜索邮件") as HTMLInputElement;

    // 未提交的内容也能清空，空格同样算输入内容。
    fireEvent.change(searchBox, { target: { value: " " } });
    expect(searchBox.parentElement?.querySelector("kbd")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "清空搜索" }));
    expect(searchBox.value).toBe("");
    expect(document.activeElement).toBe(searchBox);
    expect(api.searchMessages).not.toHaveBeenCalled();

    fireEvent.change(searchBox, { target: { value: "无匹配关键词" } });
    fireEvent.keyDown(searchBox, { key: "Enter" });
    await screen.findByText("没有找到匹配的邮件。");
    expect(screen.queryByText(/共找到 .* 封，当前已显示 .* 封/)).toBeNull();
    expect(screen.queryByText("账单邮件")).toBeNull();
    expect(searchBox.parentElement?.querySelector("kbd")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "清空搜索" }));

    expect(searchBox.value).toBe("");
    expect(document.activeElement).toBe(searchBox);
    expect(searchBox.parentElement?.querySelector("kbd")?.textContent).toBe("Ctrl+K");
    expect(screen.queryByRole("button", { name: "清空搜索" })).toBeNull();
    expect(screen.queryByText("共找到 0 封，当前已显示 0 封")).toBeNull();
    expect(screen.getByText("账单邮件")).toBeTruthy();
    expect(api.searchMessages).toHaveBeenCalledTimes(1);
  });

  it.each(["成功", "失败"])("清空后忽略晚到的搜索%s，不重新显示结果或报错", async (outcome) => {
    let finish!: () => void;
    vi.mocked(api.searchMessages).mockImplementationOnce(() => new Promise((resolve, reject) => {
      finish = () => outcome === "成功"
        ? resolve({ items: [], total: 0, offset: 0, limit: 200, deepSynced: false, deepError: null })
        : reject(new Error("旧搜索失败"));
    }));
    prepareMenu();
    await screen.findByText("账单邮件");
    const searchBox = screen.getByLabelText("搜索邮件") as HTMLInputElement;
    fireEvent.change(searchBox, { target: { value: "旧关键词" } });
    fireEvent.keyDown(searchBox, { key: "Enter" });
    expect(searchBox.getAttribute("aria-busy")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "清空搜索" }));
    expect(searchBox.getAttribute("aria-busy")).toBe("false");
    await act(async () => finish());

    expect(searchBox.value).toBe("");
    expect(screen.getByText("账单邮件")).toBeTruthy();
    expect(screen.queryByText("共找到 0 封，当前已显示 0 封")).toBeNull();
    expect(screen.queryByText(/旧搜索失败/)).toBeNull();
  });

  it("搜索框使用简短提示，输入 --help 后仍弹窗说明命令且不发搜索请求", async () => {
    render(<InboxPanel />);

    const searchBox = screen.getByLabelText("搜索邮件") as HTMLInputElement;
    expect(searchBox.placeholder).toBe(SEARCH_PLACEHOLDER);
    expect(SEARCH_PLACEHOLDER).toBe("输入 --help 查看搜索语法");
    expect(screen.queryByRole("button", { name: "搜索" })).toBeNull();
    expect(searchBox.parentElement?.querySelector("kbd")?.textContent).toBe("Ctrl+K");
    fireEvent.change(searchBox, { target: { value: "--help" } });
    const enter = createEvent.keyDown(searchBox, { key: "Enter", code: "Enter", cancelable: true });
    fireEvent(searchBox, enter);

    // 浏览器会把未拦截的回车继续交给弹窗里自动聚焦的“知道了”，造成即开即关。
    expect(enter.defaultPrevented).toBe(true);
    expect(await screen.findByRole("dialog", { name: "搜索命令" })).toBeTruthy();
    expect(screen.getByText("from:发件人")).toBeTruthy();
    expect(vi.mocked(api.searchMessages)).not.toHaveBeenCalled();
  });
});
