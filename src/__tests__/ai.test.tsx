//! Wave 7 前端安全回归：默认关闭、外发确认、三模式复用、纯文本输出。
//!
//! 渲染真实组件，只把 Tauri 命令层（../api）换成内存桩。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import MessageReader, {
  annotateEmailTranslations,
  buildReaderDocument,
  replaceEmailTranslations,
} from "../MessageReader";
import ComposePanel from "../ComposePanel";
import AiPanel from "../AiPanel";
import AiProviderDialog from "../AiProviderDialog";
import type {
  AccountInboxSummary,
  AiAuthorization,
  AiModelMap,
  AiProvider,
  AiTranslation,
  InboxMessage,
  LabelGroup,
  MailLabel,
  MessageBody,
  Proxy,
  Signature,
} from "../api";
import { api } from "../api";

vi.mock("../RichTextEditor");

vi.mock("../api", () => {
  const baseSettings = {
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
    defaultDataDir: "",
    defaultAttachmentDir: "",
    activeDataDir: "",
    activeAttachmentDir: "",
    activeAttachmentBytes: null,
    firstRun: false,
    classifyEnabled: false,
    classifyConfidencePercent: 75,
    classifySkipManualConfirm: false,
  };
  return {
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
    listProxies: vi.fn(),
    saveAiProvider: vi.fn(),
    deleteAiProvider: vi.fn(),
    testAiProvider: vi.fn(),
    listAiModelMaps: vi.fn(),
    setAiFeature: vi.fn(),
    clearAiFeature: vi.fn(),
    listAiAudit: vi.fn(),
    disableAllAi: vi.fn(),
    clearAiCache: vi.fn(),
    checkNotificationLatency: vi.fn(),
    listLabelGroups: vi.fn(),
    createLabelGroup: vi.fn(),
    renameLabelGroup: vi.fn(),
    setLabelGroupEnabled: vi.fn(),
    setLabelGroupThreshold: vi.fn(),
    deleteLabelGroup: vi.fn(),
    reorderLabelGroups: vi.fn(),
    listMailLabels: vi.fn(),
    createMailLabel: vi.fn(),
    renameMailLabel: vi.fn(),
    setMailLabelColor: vi.fn(),
    setMailLabelFallback: vi.fn(),
    deleteMailLabel: vi.fn(),
    reorderMailLabels: vi.fn(),
    setMessageLabel: vi.fn(),
    previewClassifyMessages: vi.fn(),
    classifyMessages: vi.fn(),
    getAppSettings: vi.fn(async () => ({ ...baseSettings })),
    setClassifySettings: vi.fn(async (enabled: boolean) => ({
      ...baseSettings,
      classifyEnabled: enabled,
    })),
    setClassifySkipManualConfirm: vi.fn(async (skip: boolean) => ({
      ...baseSettings,
      classifySkipManualConfirm: skip,
    })),
  },
  };
});

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
  groupResults: [],
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
  vi.mocked(api.listProxies).mockResolvedValue([]);
  vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
  vi.mocked(api.listAiAudit).mockResolvedValue([]);
  vi.mocked(api.listLabelGroups).mockResolvedValue([]);
  // MessageReader 还走旧的摊平接口（Task 12 才迁到组），这里也给个空表。
  vi.mocked(api.listMailLabels).mockResolvedValue([]);
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

  it("行内翻译切回原文后仍显示原正文", async () => {
    render(<MessageReader message={MESSAGE} aiEnabled />);
    await screen.findByText(/正文/);
    fireEvent.click(screen.getByRole("button", { name: "翻译" }));
    await screen.findByRole("dialog", { name: "确认 AI 外发" });
    fireEvent.click(screen.getByRole("button", { name: "确认并调用" }));

    fireEvent.click(await screen.findByRole("button", { name: "行内翻译" }));
    await waitFor(() => expect(frameSrcdocs()[0]).toContain("第一段译文"));
    fireEvent.click(screen.getByRole("button", { name: "切回原文" }));

    await waitFor(() => expect(frameSrcdocs()[0]).toContain("<p>第一段</p>"));
    expect(frameSrcdocs()[0]).not.toContain("第一段译文");
    expect(screen.queryByText("这封邮件没有可显示的正文。")).toBeNull();
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

  it("整封邮件套在排版表格里时逐段插译，不糊成一大段", () => {
    const html =
      "<table><tbody><tr><td><h1>Title</h1><p>First paragraph</p><p>Second paragraph</p></td></tr></tbody></table>";
    const annotated = annotateEmailTranslations(html, [
      { original: "Title", translated: "标题" },
      { original: "First paragraph", translated: "第一段译文" },
      { original: "Second paragraph", translated: "第二段译文" },
    ]);
    const parsed = document.implementation.createHTMLDocument("");
    parsed.body.innerHTML = annotated;

    const boxes = parsed.querySelectorAll('[data-em-translation="1"]');
    expect(boxes.length).toBe(3);
    expect(parsed.querySelector("h1")?.nextElementSibling?.getAttribute("data-em-translation")).toBe("1");
    expect(parsed.body.textContent).toContain("标题");
    expect(parsed.body.textContent).toContain("第一段译文");
    expect(parsed.body.textContent).toContain("第二段译文");
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

  it("整封邮件套在排版表格里时逐段换字，不把整表糊成一段", () => {
    const html =
      "<table><tbody><tr><td><h1>Title</h1><p>First paragraph</p><p>Second paragraph</p></td></tr></tbody></table>";
    const translated = replaceEmailTranslations(html, [
      { original: "Title", translated: "标题" },
      { original: "First paragraph", translated: "第一段译文" },
      { original: "Second paragraph", translated: "第二段译文" },
    ]);

    expect(translated).toContain("<h1>标题</h1>");
    expect(translated).toContain("<p>第一段译文</p>");
    expect(translated).toContain("<p>第二段译文</p>");
    expect(translated).not.toContain("First paragraph");
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
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
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

describe("AI 站点设置弹窗", () => {
  function provider(overrides: Partial<AiProvider> = {}): AiProvider {
    return {
      id: 1,
      label: "本机 Ollama",
      kind: "ollama",
      baseUrl: "http://127.0.0.1:11434",
      defaultModel: "qwen",
      models: ["qwen"],
      thinkingLevel: "off",
      enabled: true,
      hasKey: false,
      supportedFunctions: ["translate"],
      proxy: { mode: "inherit" },
      ...overrides,
    };
  }

  const PROXY_ROW: Proxy = {
    id: 5,
    label: "公司代理",
    kind: "socks5",
    host: "10.0.0.1",
    port: 1080,
    username: "",
    hasPassword: false,
  };

  it("选择通用决策站点时预填完整地址和 StartLux 模型", async () => {
    render(
      <AiProviderDialog
        open
        provider={null}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
    const dialog = await screen.findByRole("dialog");
    const kindSelect = within(dialog).getByLabelText("AI 站点类型") as HTMLSelectElement;
    const kindLabels = within(kindSelect).getAllByRole("option").map((option) => option.textContent);
    expect(kindLabels).toContain("决策模型站点");
    expect(kindLabels).not.toContain("旧版 TypeSafe jev（兼容）");
    fireEvent.change(kindSelect, { target: { value: "decision_api" } });
    expect(within(dialog).getByText("完整接口地址")).toBeTruthy();
    const addressInput = within(dialog).getByLabelText("完整接口地址") as HTMLInputElement;
    expect(addressInput.value).toBe("https://api.bamboonode.dev/v1/decisions");
    expect(
      within(dialog).getByText(
        "填写完整请求地址，例如 https://api.bamboonode.dev/v1/decisions 或 https://api.typesafe.ai/v1/systemone",
      ),
    ).toBeTruthy();
    expect((within(dialog).getByLabelText("默认模型") as HTMLInputElement).value)
      .toBe("startlux-decision-27b-autothink");
    expect(within(dialog).getByLabelText("测试模型")).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "测试连接" })).toBeTruthy();
  });

  it("编辑旧 TypeSafe 站点时保留兼容类型名称", async () => {
    render(
      <AiProviderDialog
        open
        provider={provider({
          kind: "typesafe",
          baseUrl: "https://api.typesafe.ai/v1",
          defaultModel: "jev-latest",
        })}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
    const dialog = await screen.findByRole("dialog");
    const kindSelect = within(dialog).getByLabelText("AI 站点类型") as HTMLSelectElement;
    expect(kindSelect.value).toBe("typesafe");
    expect(within(kindSelect).getAllByRole("option").map((option) => option.textContent))
      .toContain("旧版 TypeSafe jev（兼容）");

    fireEvent.change(kindSelect, { target: { value: "decision_api" } });
    expect(within(kindSelect).getAllByRole("option").map((option) => option.textContent))
      .toContain("旧版 TypeSafe jev（兼容）");
    fireEvent.change(kindSelect, { target: { value: "typesafe" } });
    expect(kindSelect.value).toBe("typesafe");
  });

  it("通用决策站点测试模型为空时不发请求", async () => {
    render(
      <AiProviderDialog
        open
        provider={null}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("AI 站点类型"), {
      target: { value: "decision_api" },
    });
    fireEvent.change(within(dialog).getByLabelText("默认模型"), { target: { value: "" } });
    fireEvent.change(within(dialog).getByLabelText("手填测试模型"), { target: { value: "   " } });
    fireEvent.click(within(dialog).getByRole("button", { name: "测试连接" }));
    expect((await within(dialog).findByRole("alert")).textContent)
      .toContain("请选择或填写测试模型");
    expect(api.testAiProvider).not.toHaveBeenCalled();
  });
  it("点「新增 AI 站点」弹出独立弹窗，Esc 能关掉", async () => {
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "新增 AI 站点" })).toBeTruthy();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("点遮罩空白关掉弹窗", async () => {
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));
    const dialog = await screen.findByRole("dialog");
    const backdrop = dialog.parentElement as HTMLElement;
    fireEvent.mouseDown(backdrop);

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("默认「跟随全局」；选「指定代理」才出现代理下拉并列出已存代理", async () => {
    vi.mocked(api.listProxies).mockResolvedValue([PROXY_ROW]);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));
    const dialog = await screen.findByRole("dialog");

    const modeSelect = within(dialog).getByLabelText("AI 站点代理策略") as HTMLSelectElement;
    expect(modeSelect.value).toBe("inherit");
    expect(within(dialog).queryByLabelText("AI 站点指定代理")).toBeNull();

    fireEvent.change(modeSelect, { target: { value: "custom" } });
    const proxySelect = within(dialog).getByLabelText("AI 站点指定代理");
    expect(within(proxySelect).getByRole("option", { name: /公司代理/ })).toBeTruthy();
  });

  it("保存新建站点时把站点级代理策略传给后端", async () => {
    vi.mocked(api.listProxies).mockResolvedValue([PROXY_ROW]);
    vi.mocked(api.saveAiProvider).mockResolvedValue(
      provider({ label: "公司站点", proxy: { mode: "custom", proxyId: 5 } }),
    );

    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));
    const dialog = await screen.findByRole("dialog");

    fireEvent.change(within(dialog).getByLabelText("AI 站点名称"), {
      target: { value: "公司站点" },
    });
    fireEvent.change(within(dialog).getByLabelText("AI 站点代理策略"), {
      target: { value: "custom" },
    });
    fireEvent.change(within(dialog).getByLabelText("AI 站点指定代理"), {
      target: { value: "5" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() => expect(api.saveAiProvider).toHaveBeenCalled());
    const draft = vi.mocked(api.saveAiProvider).mock.calls[0][0];
    expect(draft.label).toBe("公司站点");
    expect(draft.proxy).toEqual({ mode: "custom", proxyId: 5 });
    // 保存成功后弹窗自动关掉。
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("选了「指定代理」却没挑代理时拦下保存并给中文提示", async () => {
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增 AI 站点" }));
    const dialog = await screen.findByRole("dialog");

    fireEvent.change(within(dialog).getByLabelText("AI 站点名称"), {
      target: { value: "半成品" },
    });
    fireEvent.change(within(dialog).getByLabelText("AI 站点代理策略"), {
      target: { value: "custom" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent ?? "").toContain("选了「指定代理」就要挑一个具体代理");
    expect(api.saveAiProvider).not.toHaveBeenCalled();
  });

  it("站点默认折叠，折叠时可直接编辑和删除，展开后显示详情", async () => {
    vi.mocked(api.listAiProviders).mockResolvedValue([
      provider({ label: "公司站点", proxy: { mode: "custom", proxyId: 5 } }),
    ]);
    vi.mocked(api.listProxies).mockResolvedValue([PROXY_ROW]);

    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);

    const toggle = await screen.findByRole("button", { name: "展开公司站点" });
    const card = toggle.closest("article") as HTMLElement;
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(within(card).queryByText(/代理策略：/)).toBeNull();
    expect(within(card).getByRole("button", { name: "编辑" })).toBeTruthy();
    expect(within(card).getByRole("button", { name: "删除" })).toBeTruthy();
    expect(within(card).queryByRole("button", { name: "拉取模型" })).toBeNull();
    expect(
      screen.queryByText(
        "默认关闭。Key 只进系统保险箱；非本机地址必须用 HTTPS。每次真正外发前都会弹窗确认域名、模型和是否本地。",
      ),
    ).toBeNull();

    fireEvent.click(within(card).getByRole("button", { name: "编辑" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "编辑 AI 站点" })).toBeTruthy();
    expect((within(dialog).getByLabelText("AI 站点代理策略") as HTMLSelectElement).value).toBe(
      "custom",
    );
    expect((within(dialog).getByLabelText("AI 站点指定代理") as HTMLSelectElement).value).toBe("5");
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(toggle);
    expect(within(card).getByRole("button", { name: "折叠公司站点" })).toBeTruthy();
    const cardLine = within(card).getByText(/代理策略：/);
    expect(cardLine.textContent).toBe("代理策略：指定代理（公司代理）");
    expect(within(card).queryByRole("button", { name: "拉取模型" })).toBeNull();
  });

  it("多个站点可以各自展开和折叠", async () => {
    vi.mocked(api.listAiProviders).mockResolvedValue([
      provider({ id: 1, label: "站点甲" }),
      provider({
        id: 2,
        label: "站点乙",
        kind: "openai_compatible",
        baseUrl: "https://api.example.com",
        defaultModel: "gpt",
        models: ["gpt"],
      }),
    ]);

    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);

    const firstToggle = await screen.findByRole("button", { name: "展开站点甲" });
    const secondToggle = screen.getByRole("button", { name: "展开站点乙" });
    const firstCard = firstToggle.closest("article") as HTMLElement;
    const secondCard = secondToggle.closest("article") as HTMLElement;

    fireEvent.click(firstToggle);
    expect(within(firstCard).getByRole("button", { name: "折叠站点甲" })).toBeTruthy();
    expect(within(secondCard).getByRole("button", { name: "展开站点乙" })).toBeTruthy();
    expect(secondCard.textContent).not.toContain("地址：https://api.example.com");

    fireEvent.click(secondToggle);
    expect(firstCard.textContent).toContain("地址：http://127.0.0.1:11434");
    expect(secondCard.textContent).toContain("地址：https://api.example.com");

    fireEvent.click(within(firstCard).getByRole("button", { name: "折叠站点甲" }));
    expect(within(firstCard).getByRole("button", { name: "展开站点甲" })).toBeTruthy();
    expect(firstCard.textContent).not.toContain("地址：http://127.0.0.1:11434");
    expect(secondCard.textContent).toContain("地址：https://api.example.com");
  });
});

describe("通知智能识别开关", () => {
  const PROVIDER: AiProvider = {
    id: 1,
    label: "本机 Ollama",
    kind: "ollama",
    baseUrl: "http://127.0.0.1:11434",
    defaultModel: "qwen",
    models: ["qwen"],
    thinkingLevel: "off",
    enabled: true,
    hasKey: false,
    supportedFunctions: ["translate", "summary", "polish", "draft", "notification_verify"],
    proxy: { mode: "inherit" },
  };

  const VERIFY_MAP: AiModelMap = {
    function: "notification_verify",
    providerId: 1,
    model: "qwen",
    thinkingLevel: null,
    updatedAt: "2026-10-08T00:00:00Z",
  };

  it("默认关闭", async () => {
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);
    const box = (await screen.findByRole("checkbox", {
      name: "开启通知智能识别",
    })) as HTMLInputElement;
    expect(box.checked).toBe(false);
  });

  it("没有站点模型时不能打开，只给中文提示", async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={onChange} />);
    const box = (await screen.findByRole("checkbox", {
      name: "开启通知智能识别",
    })) as HTMLInputElement;

    fireEvent.click(box);

    await screen.findByText(/请先选择站点和模型/);
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: "确认开启通知智能识别" })).toBeNull();
    expect(box.checked).toBe(false);
  });

  it("配好站点后打开要弹一次风险确认，确认才落盘", async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    vi.mocked(api.listAiProviders).mockResolvedValue([PROVIDER]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([VERIFY_MAP]);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={onChange} />);
    const box = (await screen.findByRole("checkbox", {
      name: "开启通知智能识别",
    })) as HTMLInputElement;

    fireEvent.click(box);

    const dialog = await screen.findByRole("dialog", { name: "确认开启通知智能识别" });
    expect(dialog.textContent).toContain("本机 Ollama");
    expect(dialog.textContent).toContain("qwen");
    expect(dialog.textContent).toContain("是，本地服务，内容不离开这台电脑");
    expect(onChange).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "确认开启" }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(true));
    // 落盘开关的同时，也要把弹窗里展示的站点和模型存到通知识别功能上。
    expect(api.setAiFeature).toHaveBeenCalledWith("notification_verify", 1, "qwen", undefined);
  });

  it("打开开关的确认弹窗取消后不落盘", async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    vi.mocked(api.listAiProviders).mockResolvedValue([PROVIDER]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([VERIFY_MAP]);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={onChange} />);
    const box = (await screen.findByRole("checkbox", {
      name: "开启通知智能识别",
    })) as HTMLInputElement;

    fireEvent.click(box);
    const dialog = await screen.findByRole("dialog", { name: "确认开启通知智能识别" });
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "确认开启通知智能识别" })).toBeNull(),
    );
    expect(onChange).not.toHaveBeenCalled();
  });

  it("关闭开关立刻落盘，不再弹确认", async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    vi.mocked(api.listAiProviders).mockResolvedValue([PROVIDER]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([VERIFY_MAP]);
    render(<AiPanel notifyAiEnabled onNotifyAiEnabledChange={onChange} />);
    const box = (await screen.findByRole("checkbox", {
      name: "开启通知智能识别",
    })) as HTMLInputElement;
    expect(box.checked).toBe(true);

    fireEvent.click(box);

    await waitFor(() => expect(onChange).toHaveBeenCalledWith(false));
    expect(screen.queryByRole("dialog", { name: "确认开启通知智能识别" })).toBeNull();
  });

  it("通知识别的站点、模型与思考程度保存到通知智能识别功能上", async () => {
    vi.mocked(api.listAiProviders).mockResolvedValue([PROVIDER]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
    vi.mocked(api.setAiFeature).mockResolvedValue(undefined);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);

    const site = (await screen.findByLabelText("通知识别站点")) as HTMLSelectElement;
    const card = site.closest("section") as HTMLElement;
    fireEvent.change(site, { target: { value: "1" } });
    fireEvent.change(within(card).getByLabelText("通知识别模型"), {
      target: { value: "qwen2" },
    });
    fireEvent.change(within(card).getByLabelText("通知识别思考程度"), {
      target: { value: "high" },
    });
    fireEvent.click(within(card).getByRole("button", { name: "保存设置" }));

    await waitFor(() =>
      expect(api.setAiFeature).toHaveBeenCalledWith("notification_verify", 1, "qwen2", "high"),
    );
  });

  it("检查延迟先弹确认，再用当前选的模型连测三次", async () => {
    vi.mocked(api.listAiProviders).mockResolvedValue([PROVIDER]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([VERIFY_MAP]);
    vi.mocked(api.checkNotificationLatency).mockResolvedValue({
      elapsedMs: 700,
      foundCode: true,
      foundLink: false,
    });
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "检查延迟" }));

    const dialog = await screen.findByRole("dialog", { name: "确认检查延迟" });
    expect(dialog.textContent).toContain("本机 Ollama");
    expect(dialog.textContent).toContain("qwen");
    expect(dialog.textContent).toContain("是，本地服务，内容不离开这台电脑");
    // 没点「开始测试」之前，一次都不能发。
    expect(api.checkNotificationLatency).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "开始测试" }));

    await waitFor(() => expect(api.checkNotificationLatency).toHaveBeenCalledTimes(3));
    expect(api.checkNotificationLatency).toHaveBeenCalledWith(1, "qwen", undefined);
    await screen.findByText(/平均耗时：700 毫秒/);
    expect(screen.getAllByText(/认出验证码/).length).toBe(3);
  });

  it("检查延迟的确认框点取消后不发任何请求", async () => {
    vi.mocked(api.listAiProviders).mockResolvedValue([PROVIDER]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([VERIFY_MAP]);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "检查延迟" }));
    const dialog = await screen.findByRole("dialog", { name: "确认检查延迟" });
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "确认检查延迟" })).toBeNull(),
    );
    expect(api.checkNotificationLatency).not.toHaveBeenCalled();
  });

  it("没选站点时点检查延迟只给中文提示，不发任何请求", async () => {
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={async () => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "检查延迟" }));

    await screen.findByText(/请先选择站点和模型，再检查延迟。/);
    expect(screen.queryByRole("dialog", { name: "确认检查延迟" })).toBeNull();
    expect(api.checkNotificationLatency).not.toHaveBeenCalled();
  });
});

describe("邮件自动分类设置", () => {
  const openAiProvider: AiProvider = {
    id: 1,
    label: "中转站",
    kind: "openai_compatible",
    baseUrl: "https://api.example.com/v1",
    defaultModel: "gpt",
    models: ["gpt"],
    thinkingLevel: "off",
    enabled: true,
    hasKey: true,
    supportedFunctions: ["translate", "summary", "polish", "draft", "notification_verify"],
    proxy: { mode: "inherit" },
  };
  const typeSafeProvider: AiProvider = {
    id: 2,
    label: "jev",
    kind: "typesafe",
    baseUrl: "https://api.typesafe.ai/v1",
    defaultModel: "jev-latest",
    models: [],
    thinkingLevel: "off",
    enabled: true,
    hasKey: true,
    supportedFunctions: ["classify"],
    proxy: { mode: "inherit" },
  };

  const decisionProvider: AiProvider = {
    id: 3,
    label: "StartLux",
    kind: "decision_api",
    baseUrl: "https://api.bamboonode.dev/v1/decisions",
    defaultModel: "startlux-decision-27b-autothink",
    models: ["startlux-decision-27b-autothink"],
    thinkingLevel: "off",
    enabled: true,
    hasKey: true,
    supportedFunctions: ["classify"],
    proxy: { mode: "inherit" },
  };

  function prepare() {
    vi.mocked(api.listAiProviders).mockResolvedValue([
      openAiProvider,
      typeSafeProvider,
      decisionProvider,
    ]);    vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
    vi.mocked(api.listAiAudit).mockResolvedValue([]);
    vi.mocked(api.listLabelGroups).mockResolvedValue([]);
    return render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={vi.fn()} />);
  }

  it("邮件分类站点同时列出新决策站点和旧 TypeSafe", async () => {
    prepare();
    fireEvent.click(await screen.findByRole("button", { name: "展开邮件分类" }));
    const select = await screen.findByLabelText("邮件分类站点");
    const texts = within(select)
      .getAllByRole("option")
      .map((option) => option.textContent ?? "");
    expect(texts.some((text) => text.includes("StartLux"))).toBe(true);
    expect(texts.some((text) => text.includes("jev"))).toBe(true);
    expect(texts.some((text) => text.includes("中转站"))).toBe(false);
  });

  it("没有决策模型站点时明确提示去哪儿新增", async () => {
    vi.mocked(api.listAiProviders).mockResolvedValue([openAiProvider]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
    vi.mocked(api.listAiAudit).mockResolvedValue([]);
    vi.mocked(api.listLabelGroups).mockResolvedValue([]);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={vi.fn()} />);
    fireEvent.click(await screen.findByRole("button", { name: "展开邮件分类" }));
    expect(
      await screen.findByText(
        "还没有可用的决策模型站点：请先到上面的「AI 站点」里新增一个，再回来选。",
      ),
    ).toBeTruthy();
  });

  it("自动分类开关默认关闭，全局门槛不再显示且仍有一张合并卡片", async () => {
    prepare();
    const toggle = await screen.findByLabelText("开启邮件自动分类");
    expect((toggle as HTMLInputElement).checked).toBe(false);
    expect(screen.queryByLabelText("把握程度门槛")).toBeNull();
    expect(await screen.findByText("邮件自动分类 · 标签组管理")).toBeTruthy();
    expect(
      screen.queryByText(
        "组就是判断维度，一封邮件每组各出一个结果；把握门槛按组设置。标签和颜色只存在本机，不发给模型、不参与账号同步。",
      ),
    ).toBeNull();
    expect(
      screen.queryByText(
        "打开后，新邮件正文会发给所选 TypeSafe jev 站点；本地没有正文时会先自动下载到本机。模型只返回标签名和把握程度，标签和颜色只存本机。默认关闭。",
      ),
    ).toBeNull();
  });

  it("「手动分类免确认」默认关闭，勾选后写本机设置并提示", async () => {
    prepare();
    const toggle = (await screen.findByLabelText("手动分类免确认")) as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(vi.mocked(api.setClassifySkipManualConfirm)).toHaveBeenCalledWith(true),
    );
    expect(
      await screen.findByText(
        "已开启手动分类免确认：右键「立即分类」会直接外发，不再逐封确认。",
      ),
    ).toBeTruthy();
  });
});
describe("标签组管理", () => {
  const CONTENT_LABELS: MailLabel[] = [
    { id: 1, groupId: 1, name: "客户", color: "blue", sortOrder: 10, isFallback: false },
    { id: 2, groupId: 1, name: "账单", color: "orange", sortOrder: 20, isFallback: false },
    { id: 3, groupId: 1, name: "通知", color: "teal", sortOrder: 30, isFallback: false },
    { id: 4, groupId: 1, name: "推广", color: "purple", sortOrder: 40, isFallback: false },
    { id: 5, groupId: 1, name: "招聘", color: "green", sortOrder: 50, isFallback: false },
    { id: 6, groupId: 1, name: "同事", color: "indigo", sortOrder: 60, isFallback: false },
    { id: 7, groupId: 1, name: "家人", color: "pink", sortOrder: 70, isFallback: false },
    { id: 8, groupId: 1, name: "其他", color: "gray", sortOrder: 1000, isFallback: true },
  ];
  const REPLY_LABELS: MailLabel[] = [
    { id: 11, groupId: 2, name: "需要回复", color: "red", sortOrder: 10, isFallback: false },
    { id: 12, groupId: 2, name: "无需回复", color: "gray", sortOrder: 20, isFallback: false },
  ];
  const DEFAULT_GROUPS: LabelGroup[] = [
    {
      id: 1,
      name: "内容分类",
      kind: "choice",
      enabled: true,
      confidenceThreshold: 0.75,
      scoreMin: null,
      scoreMax: null,
      sortOrder: 10,
      labels: CONTENT_LABELS,
    },
    {
      id: 2,
      name: "需要我回复",
      kind: "boolean",
      enabled: false,
      confidenceThreshold: 0.5,
      scoreMin: null,
      scoreMax: null,
      sortOrder: 20,
      labels: REPLY_LABELS,
    },
    {
      id: 3,
      name: "紧急程度",
      kind: "score",
      enabled: false,
      confidenceThreshold: 0.75,
      scoreMin: 1,
      scoreMax: 5,
      sortOrder: 30,
      labels: [],
    },
  ];

  async function expandGroup(name: string) {
    fireEvent.click(await screen.findByRole("button", { name: `展开${name}` }));
  }

  function prepare() {
    vi.mocked(api.listAiProviders).mockResolvedValue([]);
    vi.mocked(api.listAiModelMaps).mockResolvedValue([]);
    vi.mocked(api.listAiAudit).mockResolvedValue([]);
    vi.mocked(api.listLabelGroups).mockResolvedValue(DEFAULT_GROUPS);
    vi.mocked(api.createMailLabel).mockResolvedValue(CONTENT_LABELS[0]);
    vi.mocked(api.renameMailLabel).mockResolvedValue(undefined);
    vi.mocked(api.setMailLabelColor).mockResolvedValue(undefined);
    vi.mocked(api.setMailLabelFallback).mockResolvedValue(undefined);
    vi.mocked(api.deleteMailLabel).mockResolvedValue(undefined);
    vi.mocked(api.reorderMailLabels).mockResolvedValue(undefined);
    vi.mocked(api.createLabelGroup).mockResolvedValue(DEFAULT_GROUPS[0]);
    vi.mocked(api.renameLabelGroup).mockResolvedValue(undefined);
    vi.mocked(api.setLabelGroupEnabled).mockResolvedValue(undefined);
    vi.mocked(api.setLabelGroupThreshold).mockResolvedValue(undefined);
    vi.mocked(api.deleteLabelGroup).mockResolvedValue(undefined);
    vi.mocked(api.reorderLabelGroups).mockResolvedValue(undefined);
    return render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={vi.fn()} />);
  }

  it("三个默认组和组内标签都列出来", async () => {
    prepare();
    expect(await screen.findByText("邮件自动分类 · 标签组管理")).toBeTruthy();
    await expandGroup("内容分类");
    await expandGroup("需要我回复");
    await expandGroup("紧急程度");
    expect(await screen.findByDisplayValue("内容分类")).toBeTruthy();
    expect(screen.getByDisplayValue("需要我回复")).toBeTruthy();
    expect(screen.getByDisplayValue("紧急程度")).toBeTruthy();
    await waitFor(() =>
      expect(document.querySelectorAll("[data-label-preview]").length).toBe(10),
    );
    expect(document.querySelector('[data-label-preview="gray"]')?.textContent).toBe("其他");
    expect(screen.getByDisplayValue("需要回复")).toBeTruthy();
    expect(screen.getByDisplayValue("无需回复")).toBeTruthy();
    expect(screen.getByLabelText("紧急程度下限")).toBeTruthy();
    expect(screen.getByLabelText("紧急程度上限")).toBeTruthy();
  });

  it("总开关关闭时提示组开关打开也不外发", async () => {
    prepare();
    expect(await screen.findByText("总开关没开，组开关打开也不外发。")).toBeTruthy();
  });

  it("新建打分题组必须填上下限", async () => {
    prepare();
    await screen.findByText("邮件自动分类 · 标签组管理");
    fireEvent.change(screen.getByLabelText("新建组名"), { target: { value: "心情" } });
    fireEvent.change(screen.getByLabelText("新建组类型"), { target: { value: "score" } });
    fireEvent.click(screen.getByRole("button", { name: "新建组" }));
    expect(vi.mocked(api.createLabelGroup)).not.toHaveBeenCalled();
    expect(await screen.findByText(/打分题要填下限和上限/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("新建组下限"), { target: { value: "1" } });
    fireEvent.change(screen.getByLabelText("新建组上限"), { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "新建组" }));
    await waitFor(() =>
      expect(vi.mocked(api.createLabelGroup)).toHaveBeenCalledWith("心情", "score", 1, 5),
    );
  });

  it("新建是否题组自动配两条标签", async () => {
    prepare();
    await screen.findByText("邮件自动分类 · 标签组管理");
    fireEvent.change(screen.getByLabelText("新建组名"), { target: { value: "要归档吗" } });
    fireEvent.change(screen.getByLabelText("新建组类型"), { target: { value: "boolean" } });
    vi.mocked(api.createLabelGroup).mockResolvedValue({
      id: 9,
      name: "要归档吗",
      kind: "boolean",
      enabled: false,
      confidenceThreshold: 0.5,
      scoreMin: null,
      scoreMax: null,
      sortOrder: 40,
      labels: [],
    });
    fireEvent.click(screen.getByRole("button", { name: "新建组" }));
    await waitFor(() =>
      expect(vi.mocked(api.createLabelGroup)).toHaveBeenCalledWith(
        "要归档吗",
        "boolean",
        undefined,
        undefined,
      ),
    );
    await waitFor(() =>
      expect(vi.mocked(api.createMailLabel)).toHaveBeenCalledWith(9, "需要回复"),
    );
    await waitFor(() =>
      expect(vi.mocked(api.createMailLabel)).toHaveBeenCalledWith(9, "无需回复"),
    );
  });

  it("每组门槛分别保存", async () => {
    prepare();
    await expandGroup("内容分类");
    await expandGroup("需要我回复");
    const content = (await screen.findByLabelText("内容分类门槛")) as HTMLInputElement;
    expect(content.value).toBe("75");
    fireEvent.change(content, { target: { value: "80" } });
    fireEvent.blur(content);
    await waitFor(() =>
      expect(vi.mocked(api.setLabelGroupThreshold)).toHaveBeenCalledWith(1, 0.8),
    );
    const reply = screen.getByLabelText("需要我回复门槛") as HTMLInputElement;
    expect(reply.value).toBe("50");
    fireEvent.change(reply, { target: { value: "60" } });
    fireEvent.blur(reply);
    await waitFor(() =>
      expect(vi.mocked(api.setLabelGroupThreshold)).toHaveBeenCalledWith(2, 0.6),
    );
  });

  it("兜底标签不能删也不能改色，可以换兜底", async () => {
    prepare();
    await expandGroup("内容分类");
    const remove = await screen.findByRole("button", { name: "删除其他" });
    expect((remove as HTMLButtonElement).disabled).toBe(true);
    const color = screen.getByLabelText("其他颜色") as HTMLSelectElement;
    expect(color.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "把客户设为兜底" }));
    await waitFor(() =>
      expect(vi.mocked(api.setMailLabelFallback)).toHaveBeenCalledWith(1, 1, true),
    );
  });

  it("组内新增标签把名称交给后端", async () => {
    prepare();
    await expandGroup("内容分类");
    const input = await screen.findByLabelText("在内容分类里新增标签");
    fireEvent.change(input, { target: { value: "项目" } });
    fireEvent.click(screen.getByRole("button", { name: "在内容分类里新增" }));
    await waitFor(() => expect(vi.mocked(api.createMailLabel)).toHaveBeenCalledWith(1, "项目"));
  });

  it("标签改名失焦提交、改色调用后端", async () => {
    prepare();
    await expandGroup("内容分类");
    const name = await screen.findByLabelText("账单标签名");
    fireEvent.change(name, { target: { value: "账单邮件" } });
    fireEvent.blur(name);
    await waitFor(() =>
      expect(vi.mocked(api.renameMailLabel)).toHaveBeenCalledWith(1, 2, "账单邮件"),
    );
    const color = screen.getByLabelText("账单颜色") as HTMLSelectElement;
    fireEvent.change(color, { target: { value: "red" } });
    await waitFor(() =>
      expect(vi.mocked(api.setMailLabelColor)).toHaveBeenCalledWith(1, 2, "red"),
    );
  });

  it("组可以改名、下移和开关", async () => {
    prepare();
    fireEvent.click(await screen.findByRole("button", { name: "下移内容分类" }));
    await waitFor(() =>
      expect(vi.mocked(api.reorderLabelGroups)).toHaveBeenCalledWith([2, 1, 3]),
    );
    const rename = screen.getByLabelText("需要我回复组名");
    fireEvent.change(rename, { target: { value: "要不要回复" } });
    fireEvent.blur(rename);
    await waitFor(() =>
      expect(vi.mocked(api.renameLabelGroup)).toHaveBeenCalledWith(2, "要不要回复"),
    );
    fireEvent.click(screen.getByLabelText("开启或关闭需要我回复"));
    await waitFor(() =>
      expect(vi.mocked(api.setLabelGroupEnabled)).toHaveBeenCalledWith(2, true),
    );
  });

  it("标签组默认收起，排序和删除排在开关前面且开关不带文字", async () => {
    const { container } = prepare();
    await screen.findByText("邮件自动分类 · 标签组管理");
    await waitFor(() => expect(container.querySelector(".ai-label-group-head")).toBeTruthy());

    // 默认收起：门槛和组内标签一开始就看不到。
    expect(screen.queryByLabelText("内容分类门槛")).toBeNull();
    expect(container.querySelector(".ai-label-group-body")).toBeNull();

    const head = container.querySelector(".ai-label-group-head") as HTMLElement;
    const controls = Array.from(
      head.querySelectorAll("button[aria-label], input[type='checkbox'][aria-label]"),
    ).map((element) => element.getAttribute("aria-label"));
    expect(controls).toEqual([
      "展开内容分类",
      "上移内容分类",
      "下移内容分类",
      "删除内容分类",
      "开启或关闭内容分类",
    ]);
    // 开关不带可见文字。
    expect(head.textContent).not.toContain("开启这一组");

    fireEvent.click(screen.getByRole("button", { name: "展开内容分类" }));
    await waitFor(() => {
      expect(screen.getByLabelText("内容分类门槛")).toBeTruthy();
    });
    expect(screen.getByRole("button", { name: "折叠内容分类" })).toBeTruthy();
  });

  it("功能级模型卡片默认折叠，可单独展开再折叠", async () => {
    const { container } = prepare();
    const cards = Array.from(
      container.querySelectorAll(".ai-feature-list > .ai-feature-card"),
    ) as HTMLElement[];
    expect(cards.length).toBeGreaterThan(1);
    expect(cards.every((card) => card.getAttribute("data-collapsed") === "true")).toBe(true);
    expect(screen.queryByLabelText("翻译站点")).toBeNull();
    expect(screen.queryByLabelText("摘要站点")).toBeNull();
    expect(screen.getByRole("button", { name: "展开翻译" })).toBeTruthy();

    const card = cards[0];
    fireEvent.click(screen.getByRole("button", { name: "展开翻译" }));
    expect(await screen.findByLabelText("翻译站点")).toBeTruthy();
    expect(card.getAttribute("data-collapsed")).toBe("false");
    // 只影响这一张卡片，别的功能仍然折叠。
    expect(screen.queryByLabelText("摘要站点")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "折叠翻译" }));
    await waitFor(() => {
      expect(screen.queryByLabelText("翻译站点")).toBeNull();
      expect(card.getAttribute("data-collapsed")).toBe("true");
    });
  });
});

describe("TypeSafe 测试模型选择", () => {
  const provider: AiProvider = {
    id: 7,
    label: "分类中转",
    kind: "typesafe",
    baseUrl: "https://relay.example.com/v1",
    defaultModel: "relay/jev-default",
    models: ["relay/jev-default", "relay/jev-fast"],
    thinkingLevel: "off",
    enabled: true,
    hasKey: true,
    supportedFunctions: ["classify"],
    proxy: { mode: "direct" },
  };

  async function prepare(overrides: Partial<AiProvider> = {}) {
    vi.mocked(api.listAiProviders).mockResolvedValue([{ ...provider, ...overrides }]);
    vi.mocked(api.testAiProvider).mockResolvedValue(["relay/jev-fast"]);
    render(<AiPanel notifyAiEnabled={false} onNotifyAiEnabledChange={vi.fn()} />);
    return (await screen.findByRole("button", { name: "展开分类中转" })).closest("article") as HTMLElement;
  }

  it("卡片默认选中站点默认模型，可以改选列表里的中转模型", async () => {
    const card = await prepare();
    fireEvent.click(within(card).getByRole("button", { name: "展开分类中转" }));
    const select = within(card).getByLabelText("测试模型") as HTMLSelectElement;
    expect(select.value).toBe("relay/jev-default");
    expect(within(select).getAllByRole("option").map((item) => item.textContent))
      .toEqual(["relay/jev-default", "relay/jev-fast", "手工填写模型编号"]);
    expect(api.testAiProvider).not.toHaveBeenCalled();

    fireEvent.change(select, { target: { value: "relay/jev-fast" } });
    fireEvent.click(within(card).getByRole("button", { name: "测试连接" }));
    await waitFor(() => expect(api.testAiProvider).toHaveBeenCalledWith(
      "typesafe", provider.baseUrl, undefined, 7, { mode: "direct" }, "relay/jev-fast",
    ));
    expect(api.saveAiProvider).not.toHaveBeenCalled();
  });

  it("没有默认模型和模型列表时可手填中转模型", async () => {
    const card = await prepare({ defaultModel: "", models: [] });
    fireEvent.click(within(card).getByRole("button", { name: "展开分类中转" }));
    fireEvent.change(within(card).getByLabelText("手填测试模型"), {
      target: { value: " relay/jev-custom " },
    });
    fireEvent.click(within(card).getByRole("button", { name: "测试连接" }));
    await waitFor(() => expect(api.testAiProvider).toHaveBeenCalledWith(
      "typesafe", provider.baseUrl, undefined, 7, { mode: "direct" }, "relay/jev-custom",
    ));
  });

  it("默认模型为空时选择列表第一项，候选会去空白和重复", async () => {
    const card = await prepare({ defaultModel: " ", models: [" ", " relay/first ", "relay/first", "relay/next"] });
    fireEvent.click(within(card).getByRole("button", { name: "展开分类中转" }));
    const select = within(card).getByLabelText("测试模型") as HTMLSelectElement;
    expect(select.value).toBe("relay/first");
    expect(within(select).getAllByRole("option").map((item) => item.textContent))
      .toEqual(["relay/first", "relay/next", "手工填写模型编号"]);
  });

  it("卡片选择手工填写后，空白模型不发请求", async () => {
    const card = await prepare();
    fireEvent.click(within(card).getByRole("button", { name: "展开分类中转" }));
    fireEvent.change(within(card).getByLabelText("测试模型"), { target: { value: "" } });
    fireEvent.change(within(card).getByLabelText("手填测试模型"), { target: { value: "   " } });
    fireEvent.click(within(card).getByRole("button", { name: "测试连接" }));
    expect((await screen.findByRole("alert")).textContent).toContain("请选择或填写测试模型");
    expect(api.testAiProvider).not.toHaveBeenCalled();
  });

  it("通用决策站点卡片也显示测试模型选择器并使用测试连接", async () => {
    const card = await prepare({
      kind: "decision_api",
      baseUrl: "https://api.bamboonode.dev/v1/decisions",
      defaultModel: "startlux-decision-27b-autothink",
      models: ["startlux-decision-27b-autothink"],
    });
    fireEvent.click(within(card).getByRole("button", { name: "展开分类中转" }));
    expect(within(card).getByText("决策模型站点")).toBeTruthy();
    expect(within(card).getByLabelText("测试模型")).toBeTruthy();
    fireEvent.click(within(card).getByRole("button", { name: "测试连接" }));
    await waitFor(() => expect(api.testAiProvider).toHaveBeenCalledWith(
      "decision_api",
      "https://api.bamboonode.dev/v1/decisions",
      undefined,
      7,
      { mode: "direct" },
      "startlux-decision-27b-autothink",
    ));
  });
  it("编辑弹窗使用当前选择测试，成功不覆盖原模型列表或默认模型", async () => {
    const card = await prepare();
    fireEvent.click(within(card).getByRole("button", { name: "编辑" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("测试模型"), {
      target: { value: "relay/jev-fast" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "测试连接" }));
    await within(dialog).findByText("连接成功，测试模型：relay/jev-fast");
    expect(api.testAiProvider).toHaveBeenCalledWith(
      "typesafe", provider.baseUrl, undefined, 7, { mode: "direct" }, "relay/jev-fast",
    );
    expect((within(dialog).getByLabelText("模型列表") as HTMLTextAreaElement).value)
      .toBe("relay/jev-default\nrelay/jev-fast");
    expect((within(dialog).getByLabelText("默认模型") as HTMLInputElement).value)
      .toBe("relay/jev-default");
    expect(api.saveAiProvider).not.toHaveBeenCalled();
  });

  it("编辑弹窗能手填模型并沿用已存密钥，重新打开恢复默认选择", async () => {
    const card = await prepare();
    fireEvent.click(within(card).getByRole("button", { name: "编辑" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("测试模型"), { target: { value: "" } });
    fireEvent.change(within(dialog).getByLabelText("手填测试模型"), {
      target: { value: " relay/custom " },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "测试连接" }));
    await within(dialog).findByText("连接成功，测试模型：relay/custom");
    expect(api.testAiProvider).toHaveBeenCalledWith(
      "typesafe", provider.baseUrl, undefined, 7, { mode: "direct" }, "relay/custom",
    );
    await waitFor(() => expect(
      (within(dialog).getByRole("button", { name: "取消" }) as HTMLButtonElement).disabled,
    ).toBe(false));
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.click(within(card).getByRole("button", { name: "编辑" }));
    const reopened = await screen.findByRole("dialog");
    expect((within(reopened).getByLabelText("测试模型") as HTMLSelectElement).value)
      .toBe("relay/jev-default");
  });

  it("编辑弹窗空模型不发请求，失败后保留所选模型便于重试", async () => {
    const card = await prepare();
    fireEvent.click(within(card).getByRole("button", { name: "编辑" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("测试模型"), { target: { value: "" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "测试连接" }));
    expect((await within(dialog).findByRole("alert")).textContent).toContain("请选择或填写测试模型");
    expect(api.testAiProvider).not.toHaveBeenCalled();
    vi.mocked(api.testAiProvider).mockRejectedValueOnce(new Error("模型不存在"));
    fireEvent.change(within(dialog).getByLabelText("手填测试模型"), {
      target: { value: "relay/other" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "测试连接" }));
    await within(dialog).findByText(/模型不存在/);
    expect((within(dialog).getByLabelText("手填测试模型") as HTMLInputElement).value).toBe("relay/other");
  });

  it("兼容站点保留原有拉取模型流程，不需要选择测试模型", async () => {
    const card = await prepare({ kind: "openai_compatible", defaultModel: "", models: [] });
    vi.mocked(api.testAiProvider).mockResolvedValue(["relay/chat"]);
    fireEvent.click(within(card).getByRole("button", { name: "编辑" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText("测试模型")).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "测试连接并拉取模型" }));
    await within(dialog).findByText("连接成功，拉到 1 个模型。");
    expect((within(dialog).getByLabelText("模型列表") as HTMLTextAreaElement).value).toBe("relay/chat");
    expect(api.testAiProvider).toHaveBeenCalledWith(
      "openai_compatible", provider.baseUrl, undefined, 7, { mode: "direct" }, undefined,
    );
  });
});
