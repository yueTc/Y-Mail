//! 内嵌图片前端回归：cid 替换、缺缓存占位、不联网、远程图片仍被拦。
//!
//! 这里用真实 MessageReader，只把 Tauri 命令层（../api）换成内存桩。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import MessageReader from "../MessageReader";
import type { InboxMessage, InlineImage, MessageBody } from "../api";
import { api } from "../api";
import { applyInlineImages, normalizeCid } from "../inlineImages";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getMessageBody: vi.fn(),
    downloadAttachment: vi.fn(),
  },
}));

const MESSAGE: InboxMessage = {
  id: 7,
  accountId: 1,
  folderId: 10,
  uid: 7,
  threadKey: "t7",
  subject: "内嵌图测试",
  fromName: "张三",
  fromAddr: "z@example.com",
  dateUtc: "2026-10-05T02:00:00Z",
  size: 1000,
  hasAttachments: true,
  isRead: false,
  isFlagged: false,
  snippet: "",
  accountEmail: "a@example.com",
  accountName: "测试账号",
  accountColor: "#3366ff",
  folderPath: "INBOX",
};

/** 一段最小 PNG 的 data URL（后端只会给受控形状）。 */
const PNG_DATA_URL = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";

function image(overrides: Partial<InlineImage> = {}): InlineImage {
  return {
    contentId: "inline-1@example.com",
    attachmentId: 9,
    mimeType: "image/png",
    size: 128,
    state: "available",
    dataUrl: PNG_DATA_URL,
    ...overrides,
  };
}

function body(overrides: Partial<MessageBody> = {}): MessageBody {
  return {
    messageId: 7,
    textPlain: null,
    html: '<p>看图</p><img src="cid:inline-1@example.com" alt="图">',
    blockedRemoteImages: 0,
    inlineImages: [],
    attachments: [],
    ...overrides,
  };
}

beforeEach(() => {
  window.localStorage.clear();
  vi.mocked(api.getMessageBody).mockResolvedValue(body());
  vi.mocked(api.downloadAttachment).mockResolvedValue("D:/downloads/图.png");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("cid 规范化与安全校验", () => {
  it("去尖括号和空白并转小写", () => {
    expect(normalizeCid("  <Img-1@Example.COM>  ")).toBe("img-1@example.com");
    expect(normalizeCid("ii_1234+ab@mail.example")).toBe("ii_1234+ab@mail.example");
  });

  it("拒绝路径穿越、注入与控制字符", () => {
    for (const bad of [
      "../../etc/passwd",
      "a/../b@x",
      "a\\..\\b@x",
      "bad\"id<x>",
      "id%2e%2e@x",
      "带中文@x",
      "",
      "<>",
      ".hidden@x",
      "a..b@x",
    ]) {
      expect(normalizeCid(bad), bad).toBeUndefined();
    }
  });
});

describe("cid → 本地图片映射", () => {
  it("已可用的内嵌图会换成受控 data URL", () => {
    const applied = applyInlineImages('<img src="cid:Inline-1@Example.com" alt="图">', [image()]);
    expect(applied.rendered).toBe(1);
    expect(applied.html).toContain(PNG_DATA_URL);
    expect(applied.html).not.toContain("cid:");
  });

  it("百分号编码的引用也能对上", () => {
    const applied = applyInlineImages('<img src="cid:inline%2D1@example.com">', [image()]);
    expect(applied.rendered).toBe(1);
  });

  it("坏形状的 data URL 不会进正文", () => {
    const applied = applyInlineImages('<img src="cid:inline-1@example.com">', [
      image({ dataUrl: "data:image/svg+xml;base64,PHN2Zz4=" }),
    ]);
    expect(applied.rendered).toBe(0);
    expect(applied.html).not.toContain("svg");
    expect(applied.html).toContain("未显示");
  });
});

describe("缺缓存与异常状态", () => {
  it("没缓存只放占位和加载按钮，渲染时不联网", () => {
    const applied = applyInlineImages('<img src="cid:inline-1@example.com">', [
      image({ state: "not-downloaded", dataUrl: null }),
    ]);
    expect(applied.rendered).toBe(0);
    expect(applied.pending).toHaveLength(1);
    expect(applied.html).not.toContain("cid:");
    expect(applied.html).toContain("未加载");
  });

  it("非图片、超大图与缺失都只给静态占位", () => {
    const unsupported = applyInlineImages('<img src="cid:x@y">', [
      image({ contentId: "x@y", state: "unsupported", dataUrl: null }),
    ]);
    expect(unsupported.rejected).toBe(1);
    expect(unsupported.html).toContain("类型不支持");

    const tooLarge = applyInlineImages('<img src="cid:x@y">', [
      image({ contentId: "x@y", state: "too-large", dataUrl: null }),
    ]);
    expect(tooLarge.html).toContain("过大");

    const missing = applyInlineImages('<img src="cid:not-there@y">', []);
    expect(missing.rejected).toBe(1);
    expect(missing.html).toContain("缺失");
  });

  it("远程图片占位属性不被当成 cid 处理，拦截规则不变", () => {
    const html = '<img data-em-original-src="https://tracker.example/p.gif" alt="像素">';
    const applied = applyInlineImages(html, [image()]);
    expect(applied.html).toBe(html);
    expect(applied.rendered).toBe(0);
    expect(applied.pending).toHaveLength(0);
  });
});

describe("读信窗格里的内嵌图交互", () => {
  it("打开邮件时不自动下载，点了按钮才下载并重新取正文", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(
      body({ inlineImages: [image({ state: "not-downloaded", dataUrl: null })] }),
    );
    render(<MessageReader message={MESSAGE} />);

    const button = await screen.findByRole("button", { name: "点一下加载" });
    expect(api.downloadAttachment).not.toHaveBeenCalled();

    fireEvent.click(button);
    await vi.waitFor(() => expect(api.downloadAttachment).toHaveBeenCalledWith(9));
    await vi.waitFor(() => expect(api.getMessageBody).toHaveBeenCalledTimes(2));
  });

  it("已可用的内嵌图直接进 iframe，且 iframe 仍不含 allow-scripts", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(body({ inlineImages: [image()] }));
    render(<MessageReader message={MESSAGE} />);

    const frame = await screen.findByTitle("邮件正文");
    expect(frame.getAttribute("sandbox") ?? "").not.toContain("allow-scripts");
    await vi.waitFor(() => expect(frame.getAttribute("srcdoc") ?? "").toContain(PNG_DATA_URL));
    expect(frame.getAttribute("srcdoc") ?? "").not.toContain("src=\"cid:");
  });
});
