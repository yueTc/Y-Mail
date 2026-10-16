//! 多邮箱同步数量回归：统一入口合计，账号入口只显示当前邮箱。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import InboxPanel from "../InboxPanel";
import { api, type SyncStatus } from "../api";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: () => ({ getVirtualItems: () => [], getTotalSize: () => 0 }),
}));

vi.mock("../api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../api")>();
  return {
    ...original,
    api: {
      ...original.api,
      listAiProviders: vi.fn().mockResolvedValue([]),
      listLabelGroups: vi.fn().mockResolvedValue([]),
      listInboxThreads: vi.fn().mockResolvedValue({ items: [], total: 0 }),
      listInboxMessages: vi.fn().mockResolvedValue({ items: [], total: 0 }),
      listInboxFolders: vi.fn().mockResolvedValue([]),
      inboxSummary: vi.fn(),
      syncStatus: vi.fn(),
    },
  };
});

function status(accountId: number, progress: number, total = progress): SyncStatus {
  return {
    accountId,
    email: `mail${accountId}@example.com`,
    state: "idle_waiting",
    stateLabel: "等待新邮件",
    progress,
    total,
    message: "",
    needsReauth: false,
    updatedAt: "2026-10-15T00:00:00Z",
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.syncStatus).mockResolvedValue([
    status(1, 1306), status(2, 200), status(3, 30, 50),
  ]);
  vi.mocked(api.inboxSummary).mockResolvedValue({
    accounts: [1, 2, 3].map((accountId) => ({
      accountId,
      email: `mail${accountId}@example.com`,
      displayName: `邮箱${accountId}`,
      color: "#3366ff",
      enabled: true,
      messageCount: 0,
      unreadCount: 0,
    })),
    totalMessages: 0,
    totalUnread: 0,
  });
});

afterEach(cleanup);

describe("多邮箱同步数量", () => {
  it("统一入口合计每个邮箱的已同步数和总数，悬停说明逐账号列出", async () => {
    render(<InboxPanel />);
    const badge = await screen.findByRole("status");
    expect(badge.textContent).toBe("等待新邮件 1536/1556");
    for (const [accountId, progress, total] of [[1, 1306, 1306], [2, 200, 200], [3, 30, 50]]) {
      expect(badge.title).toContain(`mail${accountId}@example.com`);
      expect(badge.title).toContain(`进度 ${progress}/${total}`);
    }
  });

  it("切换账号后仅显示该邮箱数量，回到统一入口恢复合计", async () => {
    render(<InboxPanel />);
    const badge = await screen.findByRole("status");
    fireEvent.click(await screen.findByRole("button", { name: "邮箱2" }));
    await waitFor(() => expect(badge.textContent).toBe("等待新邮件 200/200"));
    expect(badge.title).toContain("mail2@example.com");
    expect(badge.title).not.toContain("mail1@example.com");
    fireEvent.click(screen.getByRole("button", { name: /未读邮件/ }));
    await waitFor(() => expect(badge.textContent).toBe("等待新邮件 1536/1556"));
  });

  it("其他邮箱出错时仍显示错误和重试入口，数量继续合计", async () => {
    vi.mocked(api.syncStatus).mockResolvedValue([
      status(1, 1306),
      { ...status(2, 100, 200), state: "error", stateLabel: "同步出错", message: "连接超时" },
    ]);
    render(<InboxPanel />);
    const badge = await screen.findByRole("status");
    expect(badge.textContent).toBe("同步出错 1406/1506");
    expect(badge.classList.contains("danger")).toBe(true);
    expect(badge.title).toContain("连接超时");
    expect(screen.getByRole("button", { name: "重试" })).toBeTruthy();
  });

  it("有邮箱尚未取得总数时不把局部数量当成全部数量", async () => {
    vi.mocked(api.syncStatus).mockResolvedValue([status(1, 1306), status(2, 20, 0)]);
    render(<InboxPanel />);
    const badge = await screen.findByRole("status");
    expect(badge.textContent).toBe("等待新邮件");
    expect(badge.title).toContain("已处理 20 封");
  });

  it("没有同步状态时不显示徽标", async () => {
    vi.mocked(api.syncStatus).mockResolvedValue([]);
    render(<InboxPanel />);
    await screen.findByRole("button", { name: "邮箱1" });
    expect(screen.queryByRole("status")).toBeNull();
  });
});
