//! 统一收件箱前端回归：时间格式、文件夹名与虚拟列表的行铺平。
//!
//! 虚拟滚动在 jsdom 里拿不到真实尺寸，所以这里不渲染整个面板，
//! 只测决定列表内容的纯函数；渲染与交互另靠人工验收。

import { describe, expect, it } from "vitest";

import type { InboxFolder, InboxMessage, InboxThread } from "../api";
import { flattenInboxRows, folderLabel, formatListTime } from "../InboxPanel";

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

  it("自定义文件夹用服务器路径，已知归类用中文名", () => {
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