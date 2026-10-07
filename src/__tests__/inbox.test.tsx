//! 统一收件箱前端回归：时间格式、文件夹名与虚拟列表的行铺平。
//!
//! 虚拟滚动在 jsdom 里拿不到真实尺寸，所以这里不渲染整个面板，
//! 只测决定列表内容的纯函数；渲染与交互另靠人工验收。

import { describe, expect, it } from "vitest";

import type { InboxFolder, InboxMessage, InboxThread, SyncStatus } from "../api";
import {
  accountSidebarEntries,
  flattenInboxRows,
  folderKindMessageCount,
  folderLabel,
  formatListTime,
  inboxEmptyHint,
  inboxRowHeight,
  syncBadgeClass,
  syncBadgeText,
  syncJustSettled,
  worstSyncStatus,
} from "../InboxPanel";

function message(id: number, overrides: Partial<InboxMessage> = {}): InboxMessage {
  return {
    id,
    accountId: 1,
    folderId: 10,
    uid: id,
    threadKey: "t1",
    subject: `主题 ${id}`,
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
    ...overrides,
  };
}

function thread(accountId: number, threadKey: string, latest: InboxMessage): InboxThread {
  return {
    accountId,
    threadKey,
    messageCount: 2,
    unreadCount: 1,
    latest,
  };
}

describe("统一收件箱前端", () => {
  it("平铺模式下每封邮件一行，线程模式下折叠成一行", () => {
    const messages = [message(1), message(2, { threadKey: "t2" })];
    const threads = [
      thread(1, "t1", messages[0]),
      thread(1, "t2", messages[1]),
    ];

    const flat = flattenInboxRows([], messages, false, new Set(), new Map());
    expect(flat).toHaveLength(2);
    expect(flat[0].kind).toBe("message");

    const grouped = flattenInboxRows(threads, [], true, new Set(), new Map());
    expect(grouped).toHaveLength(2);
    expect(grouped[0].kind).toBe("thread");
    expect(grouped[0].key).toBe("1:t1");
  });

  it("展开线程时把子邮件插在对应线程后面", () => {
    const latest = message(2);
    const older = message(1);
    const threads = [thread(1, "t1", latest)];
    const expanded = new Set(["1:t1"]);
    const children = new Map([["1:t1", [latest, older]]]);

    const rows = flattenInboxRows(threads, [], true, expanded, children);
    expect(rows.map((row) => row.kind)).toEqual(["thread", "thread-message", "thread-message"]);
    expect(rows[1].key).toBe("1:t1:2");
    expect(rows[2].key).toBe("1:t1:1");
  });

  it("文件夹名以服务器展示名为准，INBOX 回退中文归类名", () => {
    const custom: InboxFolder = {
      accountId: 1,
      folderId: 5,
      fullPath: "工作/项目",
      kind: "custom",
      messageCount: 0,
      unreadCount: 0,
    };
    const inbox: InboxFolder = { ...custom, folderId: 6, fullPath: "INBOX", kind: "inbox" };
    expect(folderLabel(custom)).toBe("工作/项目");
    expect(folderLabel(inbox)).toBe("收件箱");
    // 服务器给了中文名时以它为准，否则「广告邮件」会被归类名顶成第二个「垃圾邮件」。
    const ads: InboxFolder = { ...custom, folderId: 7, fullPath: "广告邮件", kind: "junk" };
    expect(folderLabel(ads)).toBe("广告邮件");
  });

  it("昨天与今天的时间显示不同", () => {
    const now = new Date(2026, 9, 4, 18, 0, 0);
    const today = new Date(2026, 9, 4, 10, 32, 0).toISOString();
    const yesterday = new Date(2026, 9, 3, 9, 15, 0).toISOString();
    const earlier = new Date(2026, 8, 28, 9, 0, 0).toISOString();

    expect(formatListTime(today, now)).toMatch(/^\d{2}:\d{2}$/);
    expect(formatListTime(yesterday, now)).toBe("昨天");
    expect(formatListTime(earlier, now)).toBe("09-28");
    expect(formatListTime("", now)).toBe("");
  });
});

describe("左侧文件夹项", () => {
  const base: InboxFolder = {
    accountId: 1,
    folderId: 1,
    fullPath: "INBOX",
    kind: "inbox",
    messageCount: 3,
    unreadCount: 2,
  };

  it("只在收件箱后面插一条红旗邮件，别的文件夹照旧", () => {
    const folders = [
      base,
      { ...base, folderId: 2, fullPath: "已发送", kind: "sent" },
      { ...base, folderId: 3, fullPath: "垃圾邮件", kind: "junk" },
    ];
    const labels = accountSidebarEntries(folders, 1).map((entry) =>
      entry.kind === "flagged" ? "红旗邮件" : entry.folder.fullPath,
    );
    expect(labels).toEqual(["INBOX", "红旗邮件", "已发送", "垃圾邮件"]);
  });

  it("没有收件箱就补在最后，也只认本账号的文件夹", () => {
    const folders = [
      { ...base, accountId: 2, fullPath: "别的账号" },
      { ...base, folderId: 9, fullPath: "已发送", kind: "sent" },
    ];
    const labels = accountSidebarEntries(folders, 1).map((entry) =>
      entry.kind === "flagged" ? "红旗邮件" : entry.folder.fullPath,
    );
    expect(labels).toEqual(["已发送", "红旗邮件"]);
  });
});

describe("列表空文案", () => {
  it("搜索、红旗视图、空文件夹和没配账号各说各的", () => {
    expect(inboxEmptyHint({ searchMode: true, flaggedView: false, hasAccounts: true })).toBe(
      "没有找到匹配的邮件。",
    );
    expect(
      inboxEmptyHint({ searchMode: false, flaggedView: true, hasAccounts: true }),
    ).toContain("还没有标红的邮件");
    expect(inboxEmptyHint({ searchMode: false, flaggedView: false, hasAccounts: true })).toBe(
      "这里还没有邮件。",
    );
    expect(inboxEmptyHint({ searchMode: false, flaggedView: false, hasAccounts: false })).toContain(
      "先在「账号与代理」",
    );
  });
});

describe("按文件夹类型计数", () => {
  const base: InboxFolder = {
    accountId: 1,
    folderId: 1,
    fullPath: "INBOX",
    kind: "inbox",
    messageCount: 5,
    unreadCount: 2,
  };

  it("草稿和已发送按类型合计，别的类型不算进来", () => {
    const folders = [
      base,
      { ...base, folderId: 2, fullPath: "草稿箱", kind: "draft", messageCount: 3 },
      { ...base, accountId: 2, folderId: 3, fullPath: "草稿箱", kind: "draft", messageCount: 4 },
      { ...base, folderId: 4, fullPath: "已发送", kind: "sent", messageCount: 6 },
    ];
    expect(folderKindMessageCount(folders, "draft")).toBe(7);
    expect(folderKindMessageCount(folders, "sent")).toBe(6);
    expect(folderKindMessageCount(folders, "flagged")).toBe(0);
  });
});

describe("列表行高", () => {
  it("三行内容的行要留够高度，两行内容不用", () => {
    expect(inboxRowHeight("thread")).toBe(73);
    expect(inboxRowHeight("message")).toBe(73);
    expect(inboxRowHeight("thread-message")).toBe(92);
    expect(inboxRowHeight("search")).toBe(96);
  });
});

describe("同步状态徽标", () => {
  const base: SyncStatus = {
    accountId: 1,
    email: "a@example.com",
    state: "idle",
    stateLabel: "空闲",
    progress: 0,
    total: 0,
    message: "",
    needsReauth: false,
    updatedAt: "2026-10-05T00:00:00Z",
  };

  it("统一收件箱优先挑失败或需要授权的账号", () => {
    const list: SyncStatus[] = [
      { ...base, accountId: 1, state: "syncing", stateLabel: "同步中" },
      { ...base, accountId: 2, state: "error", stateLabel: "失败", message: "连接超时" },
      { ...base, accountId: 3 },
    ];
    expect(worstSyncStatus(list)?.accountId).toBe(2);
    expect(worstSyncStatus([])).toBeUndefined();
  });

  it("新账号首次同步完成或从拉取态稳定后要求刷新", () => {
    const empty = new Map<number, SyncStatus["state"]>();
    const syncingStates = new Map<number, SyncStatus["state"]>([[1, "syncing"]]);
    const settledStates = new Map<number, SyncStatus["state"]>([[1, "idle_waiting"]]);
    const syncing: SyncStatus[] = [{ ...base, state: "syncing", stateLabel: "同步中" }];
    const settled: SyncStatus[] = [{ ...base, state: "idle_waiting", stateLabel: "等待新邮件" }];

    expect(syncJustSettled(empty, settled)).toBe(true);
    expect(syncJustSettled(syncingStates, settled)).toBe(true);
    expect(syncJustSettled(settledStates, settled)).toBe(false);
    expect(syncJustSettled(empty, syncing)).toBe(false);
  });

  it("有进度时徽标带进度，失败用 danger 类", () => {
    const failed: SyncStatus = { ...base, state: "error", stateLabel: "失败", needsReauth: true };
    expect(syncBadgeClass(failed)).toBe("sync-badge danger");
    expect(syncBadgeText({ ...base, state: "syncing", stateLabel: "同步中", progress: 3, total: 10 })).toBe(
      "同步中 3/10",
    );
    expect(syncBadgeText(base)).toBe("空闲");
  });
});