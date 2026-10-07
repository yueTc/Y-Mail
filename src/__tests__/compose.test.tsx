//! 写信窗格前端回归：重复发送只发一次、临时失败重试两次后标记失败并保留草稿。
//!
//! 这里渲染真实的 `ComposePanel`，只把 Tauri 命令层（../api）换成内存桩。
//! 发送动作必须由用户点击触发，正文内容不会引起任何外发。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ComposePanel, { mergeRecipients } from "../ComposePanel";
import type { AccountInboxSummary, OutboxItem, Signature } from "../api";
import { api } from "../api";

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

/** 构造一条发件记录；只填测试关心到的字段。 */
function outboxItem(overrides: Partial<OutboxItem> = {}): OutboxItem {
  return {
    id: 1,
    accountId: 1,
    kind: "new",
    to: [{ name: "", address: "bob@example.com" }],
    cc: [],
    bcc: [],
    subject: "季度报告",
    bodyHtml: "<p>正文</p>",
    bodyText: "正文",
    inReplyTo: null,
    references: [],
    attachments: [],
    state: "queued",
    attempts: 0,
    lastError: null,
    createdAt: "2026-10-04T00:00:00Z",
    updatedAt: "2026-10-04T00:00:00Z",
    sentAt: null,
    accountEmail: "sender@example.com",
    accountDisplayName: "测试账号",
    ...overrides,
  };
}

/** 等初始加载完成：发信账号已经选中，窗口才算可用。 */
async function waitForReady() {
  await waitFor(() => {
    const select = screen.getByLabelText("发信账号") as HTMLSelectElement;
    expect(select.value).toBe("1");
  });
}

function fillDraft(subject = "季度报告") {
  fireEvent.change(screen.getByLabelText("收件人"), { target: { value: "bob@example.com" } });
  fireEvent.change(screen.getByLabelText("主题"), { target: { value: subject } });
  fireEvent.change(screen.getByLabelText("正文"), { target: { value: "正文内容" } });
}

beforeEach(() => {
  vi.mocked(api.listOutbox).mockResolvedValue([]);
  vi.mocked(api.getSignature).mockResolvedValue(EMPTY_SIGNATURE);
  vi.mocked(api.searchContacts).mockResolvedValue([]);
  vi.mocked(api.saveDraft).mockResolvedValue(1);
  vi.mocked(api.enqueueOutbox).mockResolvedValue(true);
  vi.mocked(api.retryOutbox).mockResolvedValue(true);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("写信窗格的发送闸门", () => {
  it("连点两次发送也只发一次", async () => {
    vi.mocked(api.sendOutbox).mockResolvedValue({
      attempted: 1,
      sent: 1,
      errors: [],
      reports: [{ acceptedRecipients: 1 }],
    });
    vi.mocked(api.getOutbox).mockResolvedValue(outboxItem({ state: "sent", attempts: 1 }));

    render(<ComposePanel request={{ kind: "new" }} accounts={ACCOUNTS} onClose={() => {}} />);
    await waitForReady();
    fillDraft();

    const send = screen.getByRole("button", { name: "发送" });
    fireEvent.click(send);
    fireEvent.click(send);

    await screen.findByText("已发送");
    expect(api.saveDraft).toHaveBeenCalledTimes(1);
    expect(api.enqueueOutbox).toHaveBeenCalledTimes(1);
    expect(api.sendOutbox).toHaveBeenCalledTimes(1);
  });

  it("临时失败自动重试两次后标记失败并保留草稿", async () => {
    vi.mocked(api.sendOutbox).mockResolvedValue({
      attempted: 1,
      sent: 0,
      errors: ["网络超时；已回到待发队列"],
      reports: [],
    });

    // 前两次查询还是待发，第三次（也就是第 3 轮）已经标失败。
    let lookups = 0;
    vi.mocked(api.getOutbox).mockImplementation(async () => {
      lookups += 1;
      const state = lookups >= 3 ? "failed" : "queued";
      return outboxItem({
        state,
        attempts: Math.min(lookups, 3),
        lastError: lookups >= 3 ? "网络超时；已回到待发队列" : null,
      });
    });

    // 初始加载还没有记录；发送失败后刷新发件箱，草稿仍在列表里。
    vi.mocked(api.listOutbox)
      .mockResolvedValueOnce([])
      .mockResolvedValue([
        outboxItem({ state: "failed", attempts: 3, lastError: "网络超时；已回到待发队列" }),
      ]);

    render(<ComposePanel request={{ kind: "new" }} accounts={ACCOUNTS} onClose={() => {}} />);
    await waitForReady();
    fillDraft();

    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    // 草稿仍在发件箱列表里，且被标成失败、可手动重试。
    const subject = await screen.findByText("季度报告");
    await screen.findByRole("button", { name: "重试" });
    expect(api.sendOutbox).toHaveBeenCalledTimes(3);
    const row = subject.closest("li");
    expect(row?.className).toContain("state-failed");
    expect(row?.textContent).toContain("发送失败");
    expect(row?.textContent).toContain("已尝试 3 次");
    expect(screen.getAllByText(/网络超时/).length).toBeGreaterThan(0);
  });
});

describe("通讯录带过来的预填收件人", () => {
  beforeEach(() => {
    // 这个窗格会从本地备份恢复上次没发完的草稿，先清干净再测。
    window.localStorage.clear();
  });

  it("并进收件人输入框，重复地址不再加一遍", () => {
    expect(mergeRecipients("", [{ name: "张三", address: "z@example.com" }])).toBe(
      "张三 <z@example.com>",
    );
    expect(
      mergeRecipients("李四 <l@example.com>", [
        { name: "张三", address: "z@example.com" },
        { name: "", address: "L@example.com" },
      ]),
    ).toBe("李四 <l@example.com>, 张三 <z@example.com>");
  });

  it("没有名字的联系人只写地址", () => {
    expect(mergeRecipients("", [{ name: "  ", address: " a@b.com " }])).toBe("a@b.com");
  });

  it("打开写信窗格时收件人已经填好", async () => {
    render(
      <ComposePanel
        request={{ kind: "new", to: [{ name: "张三", address: "z@example.com" }] }}
        accounts={ACCOUNTS}
        onClose={() => {}}
      />,
    );
    await waitForReady();
    await waitFor(() => {
      expect((screen.getByLabelText("收件人") as HTMLInputElement).value).toBe(
        "张三 <z@example.com>",
      );
    });
  });
});