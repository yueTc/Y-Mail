//! Wave 7 前端安全回归：默认关闭、外发确认、三模式复用、纯文本输出。
//!
//! 渲染真实组件，只把 Tauri 命令层（../api）换成内存桩。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import MessageReader, {
  annotateEmailTranslations,
  buildReaderDocument,
  replaceEmailTranslations,
} from "../MessageReader";
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

vi.mock("../RichTextEditor");

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getMessageBody: vi.fn(),
    openExternalUrl: vi.fn(),
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
  const frameSrcdocs = (): string[] =>
    Array.from(document.querySelectorAll("iframe.reader-frame")).map(
      (frame) => frame.getAttribute("srcdoc") ?? "",
    );

  it("对照左右都照邮件排版，行内原文加译文，直接只给排好版的译文", async () => {
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "翻译" }));
    await screen.findByRole("dialog", { name: "确认 AI 外发" });
    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));

    // 对照翻译：左右两栏都是 iframe，左边原邮件、右边同一封邮件的中文版。
    await waitFor(() => expect(frameSrcdocs().length).toBe(2));
    expect(api.translateMessage).toHaveBeenCalledTimes(1);
    const [original, translated] = frameSrcdocs();
    expect(original).toContain("<p>第一段</p>");
    expect(original).not.toContain("第一段译文");
    expect(translated).toContain("<p>第一段译文</p>");
    expect(translated).not.toContain("<p>第一段</p>");
    expect(translated).toContain("<p>第二段译文</p>");

    // 行内翻译：还是一封邮件，原文和译文在同一份文档里。
    fireEvent.click(screen.getByRole("button", { name: "行内翻译" }));
    await waitFor(() => expect(frameSrcdocs().length).toBe(1));
    expect(frameSrcdocs()[0]).toContain("<p>第一段</p>");
    expect(frameSrcdocs()[0]).toContain("第一段译文");
    expect(frameSrcdocs()[0]).toContain("第二段译文");

    // 直接翻译：只渲染排好版的译文文档，原文段落不再出现。
    fireEvent.click(screen.getByRole("button", { name: "直接翻译" }));
    await waitFor(() => expect(frameSrcdocs()[0]).toContain("<p>第一段译文</p>"));
    expect(frameSrcdocs().length).toBe(1);
    expect(frameSrcdocs()[0]).not.toContain("<p>第一段</p>");

    // 切回原文：译文消失，正文仍走 iframe 渲染。
    fireEvent.click(screen.getByRole("button", { name: "切回原文" }));
    await waitFor(() => expect(frameSrcdocs()[0]).toContain("<p>第一段</p>"));
    expect(frameSrcdocs()[0]).not.toContain("第一段译文");
    expect(api.translateMessage).toHaveBeenCalledTimes(1);
  });
});

describe("译文就地插回正文", () => {
  it("按段落把译文插在原文后面，图片与链接原样保留", () => {
    const html =
      '<p>Hello team</p><p><a href="https://example.com">Open</a><img src="cid:logo"></p>';
    const annotated = annotateEmailTranslations(
      html,
      [
        { original: "Hello team", translated: "你好团队" },
        { original: "Open", translated: "打开" },
      ],
    );
    expect(annotated).toContain("<p>Hello team</p>");
    expect(annotated).toContain("你好团队");
    expect(annotated).toContain("打开");
    expect(annotated).toContain('href="https://example.com"');
    expect(annotated).toContain("cid:logo");
  });

  it("模型吐出 HTML 时只当纯文字，不产生脚本节点", () => {
    const annotated = annotateEmailTranslations(
      "<p>Hello</p>",
      [{ original: "Hello", translated: '<img src=x onerror=alert(1)>' }],
    );
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = annotated;
    expect(parsed.querySelector("img")).toBeNull();
    expect(parsed.body.textContent).toContain("<img src=x onerror=alert(1)>");
  });

  it("没有对应段落时退回纯译文，不丢内容", () => {
    const documentHtml = buildReaderDocument("<p>甲</p>", {
      allowRemoteImages: false,
      dark: false,
      translations: [{ original: "找不到的段落", translated: "这段译文" }],
    });
    expect(documentHtml).toContain("<p>甲</p>");
  });
});

describe("直接翻译保留邮件排版", () => {
  it("每段文字换成译文，标题与列表还是原来的标签", () => {
    const html =
      "<h2>Daily Papers</h2><p>Hello team</p><ul><li>Alpha</li><li>Beta</li></ul>";
    const translated = replaceEmailTranslations(html, [
      { original: "Daily Papers", translated: "每日论文精选" },
      { original: "Hello team", translated: "你好团队" },
      { original: "Alpha", translated: "阿尔法" },
      { original: "Beta", translated: "贝塔" },
    ]);
    expect(translated).toContain("<h2>每日论文精选</h2>");
    expect(translated).toContain("<p>你好团队</p>");
    expect(translated).toContain("<li>阿尔法</li>");
    expect(translated).toContain("<li>贝塔</li>");
    expect(translated).not.toContain("Hello team");
    // 不再出现「译文」这两个字的标签。
    expect(translated).not.toContain(">译文<");
  });

  it("链接和图片留在原位，链接里的文字也换成中文", () => {
    const html =
      '<p>Read <a href="https://example.com">the site</a><img src="cid:logo"> now</p>';
    const translated = replaceEmailTranslations(html, [
      { original: "Read the site now", translated: "现在就去看这个站点" },
    ]);
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = translated;
    expect(parsed.querySelector("a")?.getAttribute("href")).toBe("https://example.com");
    expect(parsed.querySelector("img")?.getAttribute("src")).toBe("cid:logo");
    // 原文的英文被换掉，整段文字都是译文里的中文。
    expect(parsed.body.textContent).toContain("现在就去看这个站点");
    expect(parsed.body.textContent).not.toContain("the site");
    expect(parsed.body.textContent).not.toContain("Read");
  });

  it("长段落里的短链接不会因为分不到字而消失", () => {
    const long = "word ".repeat(40).trim();
    const html = `<p>${long} <a href="https://example.com/x">go</a> end</p>`;
    const translated = replaceEmailTranslations(html, [
      { original: `${long} go end`, translated: "这是一段很长的译文，用来验证链接不会没字。" },
    ]);
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = translated;
    const link = parsed.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.com/x");
    expect((link?.textContent ?? "").length).toBeGreaterThan(0);
    expect(parsed.body.textContent).toContain("这是一段很长的译文，用来验证链接不会没字。");
  });

  it("列表项里嵌了子列表时不动原结构，译文插在后面", () => {
    const html = "<ul><li>Item A<ul><li>Sub B</li></ul></li></ul>";
    const translated = replaceEmailTranslations(html, [
      { original: "Item A Sub B", translated: "甲项 乙子项" },
    ]);
    expect(translated).toContain("Item A");
    expect(translated).toContain("Sub B");
    expect(translated).toContain("甲项 乙子项");
  });

  it("容器里套多个段落时每段都翻译，标题和带链接的段落也不例外", () => {
    // 复刻「Daily Papers」那类邮件：整篇套在一个 <div> 里，标题、正文、列表、带链接的尾段混排。
    const html =
      "<div>" +
      "<h1>Daily Papers</h1>" +
      "<div>by community</div>" +
      "<p>Here is the selection of papers for today (6 Oct):</p>" +
      "<ul>" +
      "<li><a href=\"https://example.com/a\">Kandinsky 6.0 Video: Foundation Models for Synchronized Video and Audio Generation (113 ▲)</a></li>" +
      "<li><a href=\"https://example.com/b\">ALoDLM: Adaptive Loop Diffusion Language Models (55 ▲)</a></li>" +
      "</ul>" +
      "<p><strong>NEW</strong> - I can submit papers to be featured in Daily Papers directly: <a href=\"https://huggingface.co/papers/submit\">huggingface.co/papers/submit</a></p>" +
      "<p>Onwards and upwards,<br>AK and the research community</p>" +
      "</div>";
    const pairs: Array<[string, string]> = [
      ["Daily Papers", "每日论文精选"],
      ["by community", "由社区呈现"],
      ["Here is the selection of papers for today (6 Oct):", "这是今天（10 月 6 日）筛选的论文："],
      [
        "Kandinsky 6.0 Video: Foundation Models for Synchronized Video and Audio Generation (113 ▲)",
        "康定斯基 6.0 视频：音视频同步生成的基础模型（113 ▲）",
      ],
      ["ALoDLM: Adaptive Loop Diffusion Language Models (55 ▲)", "ALoDLM：自适应循环扩散语言模型（55 ▲）"],
      [
        "NEW - I can submit papers to be featured in Daily Papers directly: huggingface.co/papers/submit",
        "新消息——现在可以直接提交论文，入选每日论文精选：投稿链接",
      ],
      ["Onwards and upwards, AK and the research community", "继续向上，AK 与研究社区"],
    ];
    const translated = replaceEmailTranslations(
      html,
      pairs.map(([original, text]) => ({ original, translated: text })),
    );
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = translated;
    const text = parsed.body.textContent ?? "";
    for (const [, wanted] of pairs) {
      expect(text).toContain(wanted);
    }
    // 英文原文不再残留。
    expect(text).not.toContain("Foundation Models");
    expect(text).not.toContain("Here is the selection");
    expect(text).not.toContain("Onwards and upwards");
    // 链接照旧保留，文字换成中文，关键字号还在。
    expect(parsed.querySelectorAll("a").length).toBe(3);
    expect(parsed.querySelector('a[href="https://huggingface.co/papers/submit"]')).not.toBeNull();
    expect(parsed.querySelector("h1")?.textContent).toBe("每日论文精选");
    expect(text).toContain("▲");
  });

  it("原文里的空格和 <br> 差异不影响配对", () => {
    const translated = replaceEmailTranslations("<p>Second line<br>wrapped</p>", [
      { original: "Second line wrapped", translated: "第二行折行" },
    ]);
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = translated;
    expect(parsed.body.textContent).toContain("第二行折行");
    expect(parsed.body.textContent).not.toContain("Second");
  });

  it("文本对不上时按顺序兜底，译文不会掉到末尾", () => {
    const translated = replaceEmailTranslations("<p>Alpha</p><p>Beta</p>", [
      { original: "完全对不上的一段", translated: "第一段译文" },
      { original: "Beta", translated: "第二段译文" },
    ]);
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = translated;
    expect(parsed.body.textContent).toContain("第一段译文");
    expect(parsed.body.textContent).toContain("第二段译文");
    expect(parsed.querySelectorAll("p").length).toBe(2);
  });

  it("段落不够用时多出来的译文补在末尾，不会丢", () => {
    const translated = replaceEmailTranslations("<p>甲</p>", [
      { original: "甲", translated: "甲译文" },
      { original: "乙", translated: "乙译文" },
    ]);
    expect(translated).toContain("甲译文");
    expect(translated).toContain("乙译文");
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
  it("密钥输入默认遮挡，点眼睛后才显示明文", async () => {
    render(<AiPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));

    const keyInput = screen.getByLabelText("CDKey / API Key") as HTMLInputElement;
    expect(keyInput.type).toBe("password");
    expect(keyInput.value).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "显示密钥" }));
    expect(keyInput.type).toBe("text");

    fireEvent.click(screen.getByRole("button", { name: "隐藏密钥" }));
    expect(keyInput.type).toBe("password");
  });
});
