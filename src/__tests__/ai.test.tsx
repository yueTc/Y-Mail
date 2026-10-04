//! Wave 7 前端安全回归：默认关闭、外发确认、三模式复用、纯文本输出。
//!
//! 渲染真实组件，只把 Tauri 命令层（../api）换成内存桩。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import MessageReader from "../MessageReader";
import ComposePanel from "../ComposePanel";
import AiPanel from "../AiPanel";
import type {
  AccountInboxSummary,
  AiAuthorization,
  AiTranslation,
  InboxMessage,
  MessageBody,
  Signature,
} from "../api";
import { api } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getMessageBody: vi.fn(),
    downloadAttachment: vi.fn(),
    aiAuthorizationPreview: vi.fn(),
    translateMessage: vi.fn(),
    summarizeMessage: vi.fn(),
    polishText: vi.fn(),
    draftText: vi.fn(),
    listOutbox: vi.fn(),
    getSignature: vi.fn(),
    searchContacts: vi.fn(),
    saveDraft: vi.fn(),
    enqueueOutbox: vi.fn(),
    retryOutbox: vi.fn(),
    sendOutbox: vi.fn(),
    getOutbox: vi.fn(),
    deleteOutbox: vi.fn(),
    saveSignature: vi.fn(),
    listAiProviders: vi.fn(),
    saveAiProvider: vi.fn(),
    deleteAiProvider: vi.fn(),
    testAiProvider: vi.fn(),
    refreshAiProviderModels: vi.fn(),
    listAiModelMaps: vi.fn(),
    setAiFeature: vi.fn(),
    clearAiFeature: vi.fn(),
    listAiAudit: vi.fn(),
    disableAllAi: vi.fn(),
    clearAiCache: vi.fn(),
  },
}));

const MESSAGE: InboxMessage = {
  id: 42,
  accountId: 1,
  folderId: 10,
  uid: 42,
  threadKey: "t1",
  subject: "测试邮件",
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
};

const BODY: MessageBody = {
  messageId: 42,
  textPlain: "第一段",
  html: "<p>第一段</p><p>第二段</p>",
  blockedRemoteImages: 0,
  attachments: [],
};


const TRANSLATION: AiTranslation = {
  original: ["第一段", "第二段"],
  translated: ["第一段译文", "第二段译文"],
  thinkingDowngraded: false,
  fromCache: false,
};

const AUTH: AiAuthorization = {
  function: "translate",
  providerId: 1,
  providerLabel: "本机 Ollama",
  host: "127.0.0.1",
  model: "qwen",
  local: true,
  contentHash: "hash",
  authorizationToken: "token-1",
  expiresInSeconds: 300,
  fromCache: false,
};

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

beforeEach(() => {
  vi.mocked(api.getMessageBody).mockResolvedValue(BODY);
  vi.mocked(api.aiAuthorizationPreview).mockResolvedValue(AUTH);
  vi.mocked(api.translateMessage).mockResolvedValue(TRANSLATION);
  vi.mocked(api.summarizeMessage).mockResolvedValue({
    text: "这是摘要",
    thinkingDowngraded: false,
    fromCache: false,
  });
  vi.mocked(api.polishText).mockResolvedValue({
    text: "润色后的正文",
    thinkingDowngraded: false,
    fromCache: false,
  });
  vi.mocked(api.draftText).mockResolvedValue({
    text: "起草后的正文",
    thinkingDowngraded: false,
    fromCache: false,
  });
  vi.mocked(api.listOutbox).mockResolvedValue([]);
  vi.mocked(api.getSignature).mockResolvedValue(EMPTY_SIGNATURE);
  vi.mocked(api.searchContacts).mockResolvedValue([]);
  vi.mocked(api.listAiProviders).mockResolvedValue([]);
  vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
  vi.mocked(api.listAiAudit).mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("默认关闭", () => {
  it("没有启用站点时按钮显示需启用，并且不调用模型", async () => {
    render(<MessageReader message={MESSAGE} aiEnabled={false} />);
    await screen.findByText(/已拦截远程图片|正文/);

    expect(screen.getByRole("button", { name: /翻译（需启用）/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /摘要（需启用）/ })).toBeTruthy();
    expect(api.aiAuthorizationPreview).not.toHaveBeenCalled();
    expect(api.translateMessage).not.toHaveBeenCalled();
  });
});

describe("外发授权", () => {
  it("弹窗写清域名、模型和是否本地，确认后才真正调用", async () => {
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "翻译" }));

    const dialog = await screen.findByRole("dialog", { name: "确认 AI 外发" });
    expect(dialog.textContent).toContain("127.0.0.1");
    expect(dialog.textContent).toContain("qwen");
    expect(dialog.textContent).toContain("本地服务");
    expect(api.translateMessage).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));
    await waitFor(() =>
      expect(api.translateMessage).toHaveBeenCalledWith(42, "zh-CN", "token-1"),
    );
  });

  it("缓存命中不弹授权框，也不消耗令牌", async () => {
    vi.mocked(api.aiAuthorizationPreview).mockResolvedValue({
      ...AUTH,
      authorizationToken: "",
      fromCache: true,
    });
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "翻译" }));

    await waitFor(() => expect(api.translateMessage).toHaveBeenCalledWith(42, "zh-CN", ""));
    expect(screen.queryByRole("dialog", { name: "确认 AI 外发" })).toBeNull();
  });
});

describe("翻译三种模式", () => {
  it("切换对照、行内、直接、切回原文都不会重复请求模型", async () => {
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "翻译" }));
    await screen.findByRole("dialog", { name: "确认 AI 外发" });
    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));
    await screen.findByText("第一段译文");
    expect(api.translateMessage).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "行内翻译" }));
    await screen.findByText("第一段译文");
    fireEvent.click(screen.getByRole("button", { name: "直接翻译" }));
    await screen.findByText("第一段译文");
    fireEvent.click(screen.getByRole("button", { name: "切回原文" }));
    await screen.findByTitle("邮件正文");

    expect(api.translateMessage).toHaveBeenCalledTimes(1);
  });
});

describe("模型输出安全", () => {
  it("模型返回 HTML 时按纯文本渲染，不产生脚本节点", async () => {
    vi.mocked(api.aiAuthorizationPreview).mockResolvedValue({
      ...AUTH,
      function: "summary",
    });
    vi.mocked(api.summarizeMessage).mockResolvedValue({
      text: '<script>window.__pwned = true</script><a href="https://evil.example">点我</a>',
      thinkingDowngraded: false,
      fromCache: false,
    });
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "摘要" }));
    await screen.findByRole("dialog", { name: "确认 AI 外发" });
    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));

    await screen.findByText(/<script>/);
    expect(document.querySelector("script[data-pwned]")).toBeNull();
    expect((window as unknown as { __pwned?: boolean }).__pwned).toBeUndefined();
    expect(screen.queryByRole("link", { name: "点我" })).toBeNull();
  });

  it("思考程度降级时显示已降级提示", async () => {
    vi.mocked(api.translateMessage).mockResolvedValue({
      ...TRANSLATION,
      thinkingDowngraded: true,
    });
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "翻译" }));
    await screen.findByRole("dialog", { name: "确认 AI 外发" });
    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));

    await screen.findByText(/该模型不支持所选思考程度/);
  });
});

describe("写信 AI 外发闸门", () => {
  it("润色未确认前不会发送正文", async () => {
    render(<ComposePanel request={{ kind: "new" }} accounts={ACCOUNTS} onClose={() => {}} aiEnabled />);
    await waitFor(() => {
      const select = screen.getByLabelText("发信账号") as HTMLSelectElement;
      expect(select.value).toBe("1");
    });
    fireEvent.change(screen.getByLabelText("正文"), { target: { value: "要润色的正文" } });
    fireEvent.click(screen.getByRole("button", { name: "AI 润色" }));

    const dialog = await screen.findByRole("dialog", { name: "确认 AI 外发" });
    expect(dialog.textContent).toContain("127.0.0.1");
    expect(dialog.textContent).toContain("qwen");
    expect(api.polishText).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));
    await waitFor(() => expect(api.polishText).toHaveBeenCalledWith("要润色的正文", "token-1"));
    await waitFor(() => {
      const body = screen.getByLabelText("正文") as HTMLTextAreaElement;
      expect(body.value).toBe("润色后的正文");
    });
  });
});

describe("AI 设置页密钥", () => {
  it("密钥输入用密码框且初始不回显", async () => {
    render(<AiPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));

    const keyInput = screen.getByLabelText("CDKey / API Key") as HTMLInputElement;
    expect(keyInput.type).toBe("password");
    expect(keyInput.value).toBe("");
  });
});