import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import App from "../App";
import { api, type InboxMessage } from "../api";

const events = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    events.set(name, handler);
    return () => { if (events.get(name) === handler) events.delete(name); };
  }),
}));
vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: () => ({ getVirtualItems: () => [], getTotalSize: () => 0 }),
}));
vi.mock("../ContactsWorkspace", () => ({ default: () => <div>通讯录内容</div> }));
vi.mock("../SettingsWorkspace", () => ({ default: () => <div>设置内容</div> }));
vi.mock("../FirstRunWizard", () => ({ default: () => null }));
vi.mock("../MessageReader", () => ({
  default: ({ message }: { message?: InboxMessage }) => <div data-testid="notification-reader">{message?.subject}</div>,
}));
vi.mock("../ComposePanel", () => ({
  default: ({ active = true }: { active?: boolean }) => {
    const [draft, setDraft] = useState("");
    return <div hidden={!active}><input aria-label="写信草稿" value={draft} onChange={(event) => setDraft(event.target.value)} /></div>;
  },
}));
vi.mock("../api", () => ({
  describeError: (error: unknown) => error instanceof Error ? error.message : String(error),
  api: {
    getAppSettings: vi.fn(async () => ({ firstRun: false, autoCheckUpdate: false, updateCheckIntervalHours: 24 })),
    inboxSummary: vi.fn(async () => ({ accounts: [], totalUnread: 0, totalMessages: 0 })),
    listInboxFolders: vi.fn(async () => []),
    listInboxMessages: vi.fn(async () => ({ items: [], total: 0, offset: 0, limit: 200 })),
    listInboxThreads: vi.fn(async () => ({ items: [], total: 0, offset: 0, limit: 200 })),
    listAiProviders: vi.fn(async () => []),
    listLabelGroups: vi.fn(async () => []),
    syncStatus: vi.fn(async () => []),
    getInboxMessage: vi.fn(),
    setMessageRead: vi.fn(async () => undefined),
  },
}));

function message(id: number): InboxMessage {
  return {
    id, accountId: 1, folderId: 10, uid: id, threadKey: `thread-${id}`,
    subject: `通知邮件 ${id}`, fromName: "发件人", fromAddr: "sender@example.com",
    dateUtc: "2026-10-15T00:00:00Z", size: 100, hasAttachments: false,
    isRead: true, isFlagged: false, snippet: "", accountEmail: "me@example.com",
    accountName: "测试账号", accountColor: "#3366ff", folderPath: "INBOX", groupResults: [],
  } as InboxMessage;
}

async function start() {
  render(<App />);
  await waitFor(() => expect(events.has("inbox:open-message")).toBe(true));
}

async function clickNotification(messageId: unknown) {
  await act(async () => { events.get("inbox:open-message")?.({ payload: { messageId } }); });
}

beforeEach(() => {
  vi.clearAllMocks();
  events.clear();
  vi.mocked(api.getInboxMessage).mockImplementation(async (id) => message(id));
});
afterEach(cleanup);

describe("通知点击直达邮件", () => {
  it.each(["设置", "通讯录"])("从%s切回收件箱并打开目标邮件", async (page) => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: page }));
    await clickNotification(42);
    await waitFor(() => expect(screen.getByRole("button", { name: "收件箱" }).getAttribute("aria-current")).toBe("page"));
    expect(api.getInboxMessage).toHaveBeenCalledWith(42);
    expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 42");
    expect(screen.getByTestId("notification-reader").closest("[hidden]")).toBeNull();
  });

  it("通知邮件不在当前列表时也能打开，并标记已读", async () => {
    vi.mocked(api.getInboxMessage).mockResolvedValue({ ...message(42), isRead: false });
    await start();
    fireEvent.click(screen.getByRole("button", { name: "只看未读" }));
    await clickNotification(42);
    await waitFor(() => expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 42"));
    expect(api.setMessageRead).toHaveBeenCalledWith(42, true);
  });

  it("切换筛选不会反复解绑打开邮件监听", async () => {
    await start();
    const count = () => vi.mocked(listen).mock.calls.filter(([name]) => name === "inbox:open-message").length;
    expect(count()).toBe(1);
    fireEvent.click(screen.getByRole("button", { name: "只看未读" }));
    await clickNotification(42);
    expect(count()).toBe(1);
  });

  it("忽略无效编号", async () => {
    await start();
    for (const id of ["42", -1, 0, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) await clickNotification(id);
    expect(api.getInboxMessage).not.toHaveBeenCalled();
  });

  it("目标已删除时提示原因", async () => {
    vi.mocked(api.getInboxMessage).mockResolvedValue(null);
    await start();
    await clickNotification(42);
    expect(await screen.findByText("这封邮件已不存在，可能已被删除或重新同步。" )).toBeTruthy();
    expect(screen.getByTestId("notification-reader").textContent).toBe("");
  });

  it("连续点击时后点击的邮件优先，旧请求晚返回也不覆盖", async () => {
    let resolveFirst!: (value: InboxMessage) => void;
    vi.mocked(api.getInboxMessage).mockImplementation((id) => id === 1
      ? new Promise<InboxMessage>((resolve) => { resolveFirst = resolve; })
      : Promise.resolve(message(id)));
    await start();
    await clickNotification(1);
    await clickNotification(2);
    await waitFor(() => expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 2"));
    await act(async () => resolveFirst(message(1)));
    expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 2");
  });

  it("前一封标已读迟到失败时也不覆盖后点击的邮件", async () => {
    let rejectRead!: (reason: Error) => void;
    vi.mocked(api.getInboxMessage).mockImplementation(async (id) => ({ ...message(id), isRead: id !== 1 }));
    vi.mocked(api.setMessageRead).mockImplementationOnce(() => new Promise((_, reject) => { rejectRead = reject; }));
    await start();
    await clickNotification(1);
    await waitFor(() => expect(api.setMessageRead).toHaveBeenCalledWith(1, true));
    await clickNotification(2);
    await act(async () => rejectRead(new Error("同步失败")));
    expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 2");
  });

  it("读取目标失败时显示原因，下一次点击成功后清掉提示", async () => {
    vi.mocked(api.getInboxMessage).mockRejectedValueOnce(new Error("读取失败"));
    await start();
    await clickNotification(1);
    expect(await screen.findByText("读取失败")).toBeTruthy();
    await clickNotification(2);
    expect(screen.queryByText("读取失败")).toBeNull();
    expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 2");
  });

  it("打开通知邮件后可继续写信，已输入草稿保持原样", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "写邮件" }));
    fireEvent.change(screen.getByLabelText("写信草稿"), { target: { value: "尚未保存的正文" } });
    await clickNotification(42);
    await waitFor(() => expect(screen.getByTestId("notification-reader").textContent).toBe("通知邮件 42"));
    expect(screen.getByLabelText("写信草稿").closest("[hidden]")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "继续写信" }));
    expect((screen.getByLabelText("写信草稿") as HTMLInputElement).value).toBe("尚未保存的正文");
    expect(screen.getByLabelText("写信草稿").closest("[hidden]")).toBeNull();
  });
});
