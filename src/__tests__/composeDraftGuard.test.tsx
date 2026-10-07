//! 写信草稿保护回归：关闭确认三选一、自动备份、重开恢复、发送失败保留草稿。

import { open } from "@tauri-apps/plugin-dialog";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ComposePanel from "../ComposePanel";
import type { AccountInboxSummary, OutboxItem, Signature } from "../api";
import { api } from "../api";
import { COMPOSE_DRAFT_STORAGE_KEY, deserializeComposeDraft } from "../composeDraft";

vi.mock("../RichTextEditor");

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

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
    bodyText: "正文内容",
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

function renderPanel(onClose: () => void = () => {}) {
  return render(<ComposePanel request={{ kind: "new" }} accounts={ACCOUNTS} onClose={onClose} />);
}

beforeEach(() => {
  window.localStorage.clear();
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
  vi.useRealTimers();
  window.localStorage.clear();
});

describe("写信草稿保护", () => {
  it("没有改动时关闭不弹确认框", async () => {
    const onClose = vi.fn();
    renderPanel(onClose);
    await waitForReady();

    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog", { name: "关闭写信窗格" })).toBeNull();
  });

  it("有未保存内容时关闭先弹三选一，取消后内容还在", async () => {
    const onClose = vi.fn();
    renderPanel(onClose);
    await waitForReady();
    fillDraft();

    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.getByRole("dialog", { name: "关闭写信窗格" })).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "取消关闭" }));
    expect(screen.queryByRole("dialog", { name: "关闭写信窗格" })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect((screen.getByLabelText("正文") as HTMLTextAreaElement).value).toBe("正文内容");

    // 再点一次关闭仍然会弹，说明取消没有破坏保护。
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.getByRole("dialog", { name: "关闭写信窗格" })).toBeTruthy();
  });

  it("选保存草稿会先调用 saveDraft 再关闭", async () => {
    const onClose = vi.fn();
    renderPanel(onClose);
    await waitForReady();
    fillDraft();

    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    fireEvent.click(screen.getByRole("button", { name: "保存草稿" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(api.saveDraft).toHaveBeenCalledTimes(1);
    expect(window.localStorage.getItem(COMPOSE_DRAFT_STORAGE_KEY)).toBeNull();
  });

  it("选放弃修改会直接关闭并清掉本地备份", async () => {
    const onClose = vi.fn();
    renderPanel(onClose);
    await waitForReady();
    fillDraft();
    window.localStorage.setItem(COMPOSE_DRAFT_STORAGE_KEY, "{\"kind\":\"new\"}");

    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    fireEvent.click(screen.getByRole("button", { name: "放弃修改" }));

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(window.localStorage.getItem(COMPOSE_DRAFT_STORAGE_KEY)).toBeNull();
  });

  it("编辑停止约 1 秒后写入本地备份，重开写信能恢复", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const first = renderPanel();
    await waitForReady();
    fillDraft();

    vi.advanceTimersByTime(1200);
    await waitFor(() => expect(window.localStorage.getItem(COMPOSE_DRAFT_STORAGE_KEY)).not.toBeNull());
    const stored = deserializeComposeDraft(window.localStorage.getItem(COMPOSE_DRAFT_STORAGE_KEY));
    expect(stored?.subject).toBe("季度报告");
    expect(stored?.bodyText).toBe("正文内容");

    // 卸载再重开，等同用户关掉后再点“写邮件”。
    first.unmount();
    renderPanel();
    await waitFor(() => expect((screen.getByLabelText("正文") as HTMLTextAreaElement).value).toBe("正文内容"));
    expect((screen.getByLabelText("主题") as HTMLInputElement).value).toBe("季度报告");
    expect(screen.getByText("草稿")).toBeTruthy();
  });

  it("Esc 在有未保存内容时弹确认，在确认框里再按 Esc 只关确认框", async () => {
    const onClose = vi.fn();
    renderPanel(onClose);
    await waitForReady();
    fillDraft();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "关闭写信窗格" })).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "关闭写信窗格" })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("发送失败草稿固化", () => {
  it("发送失败后草稿和正文仍保留，重试按钮可用", async () => {
    vi.mocked(api.sendOutbox).mockResolvedValue({
      attempted: 1,
      sent: 0,
      errors: ["网络超时；已回到待发队列"],
      reports: [],
    });
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
    vi.mocked(api.listOutbox)
      .mockResolvedValueOnce([])
      .mockResolvedValue([
        outboxItem({ state: "failed", attempts: 3, lastError: "网络超时；已回到待发队列" }),
      ]);

    renderPanel();
    await waitForReady();
    fillDraft();

    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    await screen.findByRole("button", { name: "重试" });
    expect((screen.getByLabelText("正文") as HTMLTextAreaElement).value).toBe("正文内容");
    expect(api.sendOutbox).toHaveBeenCalledTimes(3);
  });

  it("发送失败时保留草稿和附件选择", async () => {
    vi.mocked(api.sendOutbox).mockResolvedValue({
      attempted: 1,
      sent: 0,
      errors: ["网络超时；已回到待发队列"],
      reports: [],
    });
    let lookups = 0;
    vi.mocked(api.getOutbox).mockImplementation(async () => {
      lookups += 1;
      return outboxItem({
        state: lookups >= 3 ? "failed" : "queued",
        attempts: Math.min(lookups, 3),
        attachments: [{ path: "C:/tmp/report.pdf", filename: "report.pdf" }],
      });
    });
    vi.mocked(api.listOutbox)
      .mockResolvedValueOnce([])
      .mockResolvedValue([
        outboxItem({
          state: "failed",
          attempts: 3,
          lastError: "网络超时；已回到待发队列",
          attachments: [{ path: "C:/tmp/report.pdf", filename: "report.pdf" }],
        }),
      ]);

    renderPanel();
    await waitForReady();
    fillDraft();
    vi.mocked(open).mockResolvedValue("C:/tmp/report.pdf");
    fireEvent.click(screen.getByRole("button", { name: "附件" }));
    await screen.findByText("report.pdf");

    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await screen.findByRole("button", { name: "重试" });

    // 附件仍在界面里，且最后一次保存的草稿带着附件，没有静默清空。
    expect(screen.getByText("report.pdf")).toBeTruthy();
    const lastDraft = vi.mocked(api.saveDraft).mock.calls.at(-1)?.[0];
    expect(lastDraft?.attachments).toEqual([
      { path: "C:/tmp/report.pdf", filename: "report.pdf" },
    ]);
    expect((screen.getByLabelText("正文") as HTMLTextAreaElement).value).toBe("正文内容");
  });
});