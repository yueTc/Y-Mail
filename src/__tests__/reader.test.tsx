//! 读信窗格前端回归：iframe 沙箱、远程图片放行与附件警示。
//!
//! 这里渲染真实的 `MessageReader`，只把 Tauri 命令层（../api）换成内存桩。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import MessageReader, {
  bindReaderLinks,
  buildReaderDocument,
  externalReaderLink,
  isExecutableAttachment,
} from "../MessageReader";
import type { InboxMessage, MessageAttachment, MessageBody } from "../api";
import { api } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getMessageBody: vi.fn(),
    downloadAttachment: vi.fn(),
    downloadExternalAttachment: vi.fn(),
    openDownloadedFile: vi.fn(),
    openDownloadedFileDir: vi.fn(),
    openExternalUrl: vi.fn(),
    rememberRemoteSender: vi.fn(),
    listTrustedRemoteSenders: vi.fn(),
    forgetRemoteSender: vi.fn(),
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
  hasAttachments: true,
  isRead: false,
  isFlagged: false,
  snippet: "",
  accountEmail: "a@example.com",
  accountName: "测试账号",
  accountColor: "#3366ff",
  folderPath: "INBOX",
};

/** 网易 163 超大附件的正文片段：正文里只有下载链接，没有 MIME 附件。 */
function bigAttachHtml(expiry: string): string {
  return [
    '<div style="padding:4px">',
    '<div style="font-size:14px"><b>从网易163邮箱发来的超大附件</b>',
    '<a class="bigattach-mailmaster-download" href="http://u.163.com/RcUwvU56g">推荐客户端极速下载</a></div>',
    '<div style="padding:4px"><div style="height:36px;padding:6px 4px">',
    '<div style="float:left;width:36px"><a href="https://mail.163.com/large-attachment-download/index.html?file=djAyZG1meTh2"></a></div>',
    '<div><div style="font-size:12px">',
    '<a href="https://mail.163.com/large-attachment-download/index.html?file=djAyZG1meTh2">虚拟.wav</a>',
    `<span style="color:#bbb"> (80.96M, ${expiry} 到期)</span></div>`,
    '<div style="font-size:12px"><a href="https://mail.163.com/large-attachment-download/index.html?file=djAyZG1meTh2">下载</a></div>',
    '</div></div></div></div>',
  ].join("");
}

/** 还没过期的一条（2099 年到期），用来测下载。 */
const BIG_ATTACH_HTML = bigAttachHtml("2099年12月31日 23:59");
/** 正文里写着早就过期的一条。 */
const BIG_ATTACH_EXPIRED_HTML = bigAttachHtml("2001年1月1日 0:00");

const ATTACHMENT: MessageAttachment = {
  id: 9,
  messageId: 42,
  partIndex: 2,
  filename: "报告.pdf",
  mimeType: "application/pdf",
  size: 2048,
  contentId: null,
  isInline: false,
  localPath: null,
  state: "pending",
};

function body(overrides: Partial<MessageBody> = {}): MessageBody {
  return {
    messageId: 42,
    textPlain: null,
    html: '<p>正文</p><img data-em-original-src="https://tracker.example/p.gif" alt="图">',
    blockedRemoteImages: 1,
    attachments: [ATTACHMENT],
    ...overrides,
  };
}

beforeEach(() => {
  window.localStorage.clear();
  vi.mocked(api.getMessageBody).mockResolvedValue(body());
  vi.mocked(api.downloadAttachment).mockResolvedValue("D:/downloads/报告.pdf");
  vi.mocked(api.rememberRemoteSender).mockResolvedValue(["z@example.com"]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("读信窗格的沙箱与远程图片", () => {
  it("iframe 带 sandbox：接管链接但不放脚本", async () => {
    render(<MessageReader message={MESSAGE} />);
    await screen.findByText(/已拦截远程图片/);

    const frame = await screen.findByTitle("邮件正文");
    expect(frame.tagName.toLowerCase()).toBe("iframe");
    const sandbox = frame.getAttribute("sandbox");
    expect(sandbox).not.toBeNull();
    expect(sandbox ?? "").not.toContain("allow-scripts");
    // 外层要拿得到正文文档才能接管链接点击，所以必须同源。
    expect(sandbox ?? "").toContain("allow-same-origin");
  });

  it("正文里的链接交给系统浏览器：只认绝对 http / https / mailto", () => {
    expect(externalReaderLink("https://example.com/a")).toBe("https://example.com/a");
    expect(externalReaderLink("HTTP://Example.com/a")).toBe("HTTP://Example.com/a");
    expect(externalReaderLink("mailto:a@example.com")).toBe("mailto:a@example.com");
    expect(externalReaderLink("  https://example.com/a  ")).toBe("https://example.com/a");
    // 页内锚点、相对地址和别的协议都不外开。
    expect(externalReaderLink("#top")).toBeUndefined();
    expect(externalReaderLink("/relative")).toBeUndefined();
    expect(externalReaderLink("example.com")).toBeUndefined();
    expect(externalReaderLink("javascript:alert(1)")).toBeUndefined();
    expect(externalReaderLink("file:///C:/Windows")).toBeUndefined();
    expect(externalReaderLink("")).toBeUndefined();
    expect(externalReaderLink(null)).toBeUndefined();
  });

  it("点正文里的链接会拦下默认跳转并交给系统浏览器", () => {
    const doc = document.implementation.createHTMLDocument("正文");
    doc.body.innerHTML =
      '<p><a href="https://example.com/a">外链</a><a href="#top">页内</a></p>';
    const opened: string[] = [];
    const unbind = bindReaderLinks(doc, (url) => opened.push(url));

    const [outside, inside] = Array.from(doc.querySelectorAll("a"));
    const outsideClick = new MouseEvent("click", { bubbles: true, cancelable: true });
    outside.dispatchEvent(outsideClick);
    expect(opened).toEqual(["https://example.com/a"]);
    expect(outsideClick.defaultPrevented).toBe(true);

    // 页内锚点照旧留给正文自己跳。
    const insideClick = new MouseEvent("click", { bubbles: true, cancelable: true });
    inside.dispatchEvent(insideClick);
    expect(opened).toEqual(["https://example.com/a"]);
    expect(insideClick.defaultPrevented).toBe(false);

    // 解绑之后不再接管。
    unbind();
    outside.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(opened).toEqual(["https://example.com/a"]);
  });

  it("默认拦截远程图片，放行后才把地址还给界面", async () => {
    render(<MessageReader message={MESSAGE} />);
    fireEvent.click(await screen.findByRole("button", { name: "本封放行远程图片" }));

    await vi.waitFor(() => expect(api.getMessageBody).toHaveBeenCalledTimes(2));
    expect(vi.mocked(api.getMessageBody).mock.calls[1][1]).toBe(true);
    await screen.findByText(/本封已放行远程图片/);
  });

  it("可以记住发件人，之后自动放行", async () => {
    render(<MessageReader message={MESSAGE} />);
    fireEvent.click(await screen.findByRole("button", { name: "以后这个发件人都自动显示" }));
    await vi.waitFor(() => expect(api.rememberRemoteSender).toHaveBeenCalledWith(42));
    await screen.findByText(/已记住这个发件人/);
  });

  it("后端自动放行时不再显示拦截条，并把 http 加进 CSP", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(body({ remoteImagesAllowed: true }));
    render(<MessageReader message={MESSAGE} />);
    const frame = await screen.findByTitle("邮件正文");
    await vi.waitFor(() => expect(frame.getAttribute("srcdoc") ?? "").toContain("https:"));
    expect(screen.queryByText(/已拦截远程图片/)).toBeNull();
  });

  it("文档 CSP 默认不放行远程图片，放行后才加 http/https", () => {
    const locked = buildReaderDocument("<p>正文</p>", { allowRemoteImages: false, dark: false });
    expect(locked).toContain("default-src 'none'");
    expect(locked).not.toContain("http:");
    expect(locked).not.toContain("https:");
    expect(locked).not.toContain("allow-scripts");

    const allowed = buildReaderDocument('<img src="https://a/b.png">', {
      allowRemoteImages: true,
      dark: true,
    });
    expect(allowed).toContain("https:");
    expect(allowed).toContain("http:");
  });
});

describe("读信窗格的附件与深色模式", () => {
  it("可执行附件会标红警示", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(
      body({
        attachments: [
          {
            ...ATTACHMENT,
            id: 10,
            filename: "木马.exe",
            mimeType: "application/x-msdownload",
          },
        ],
      }),
    );
    render(<MessageReader message={MESSAGE} />);
    await screen.findByText("木马.exe");
    expect(screen.getByText(/可执行文件/)).toBeTruthy();
    expect(isExecutableAttachment({ ...ATTACHMENT, id: 10, filename: "木马.exe" })).toBe(true);
  });

  it("深色模式开关已搬走，读信窗格只按本地偏好渲染", async () => {
    window.localStorage.setItem("ymail.reader-theme", "dark");
    render(<MessageReader message={MESSAGE} />);
    await screen.findByTitle("邮件正文");
    expect(screen.queryByLabelText("读信深色模式")).toBeNull();
    expect(document.querySelector(".reader-dark")).toBeTruthy();
  });

  it("有附件时显示上下拖动条，附件为空时不显示", async () => {
    render(<MessageReader message={MESSAGE} />);
    const separator = await screen.findByRole("separator", { name: "附件区高度" });
    expect(separator.getAttribute("aria-orientation")).toBe("horizontal");

    cleanup();
    vi.mocked(api.getMessageBody).mockResolvedValue(body({ attachments: [] }));
    render(<MessageReader message={MESSAGE} />);
    await screen.findByTitle("邮件正文");
    expect(screen.queryByRole("separator", { name: "附件区高度" })).toBeNull();
    expect(screen.queryByRole("region", { name: "附件" })).toBeNull();
    expect(screen.queryByText(/附件（0）/)).toBeNull();
  });

  it("网易超大附件能直接下载，下完给出打开文件和打开所在位置", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(
      body({ attachments: [], html: BIG_ATTACH_HTML }),
    );
    vi.mocked(api.downloadExternalAttachment).mockResolvedValue({
      path: "D:/downloads/虚拟.wav",
      filename: "虚拟.wav",
      size: 1024,
    });
    render(<MessageReader message={MESSAGE} />);
    await screen.findByText("虚拟.wav");
    expect(screen.getByText("外部大附件（1）")).toBeTruthy();
    expect(screen.getByText("80.96M · 2099年12月31日 23:59 到期")).toBeTruthy();
    expect(screen.getByRole("button", { name: "复制链接" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "下载" }));
    await waitFor(() => expect(api.downloadExternalAttachment).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("已保存：D:/downloads/虚拟.wav")).toBeTruthy();
    expect(screen.getByRole("button", { name: "打开文件" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "打开所在位置" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "打开文件" }));
    await waitFor(() =>
      expect(api.openDownloadedFile).toHaveBeenCalledWith("D:/downloads/虚拟.wav"),
    );
    fireEvent.click(screen.getByRole("button", { name: "打开所在位置" }));
    await waitFor(() =>
      expect(api.openDownloadedFileDir).toHaveBeenCalledWith("D:/downloads/虚拟.wav"),
    );
  });

  it("正文里写着已经过期的超大附件，按钮变成已过期且点不动", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(
      body({ attachments: [], html: BIG_ATTACH_EXPIRED_HTML }),
    );
    render(<MessageReader message={MESSAGE} />);
    await screen.findByText("虚拟.wav");
    // 到期标记是正文里算出来的，按钮同时锁住。
    expect(document.querySelector(".attachment-expired")).toBeTruthy();
    const button = screen.getByRole("button", { name: "已过期" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });

  it("服务器说过期时，下载失败就地显示已过期", async () => {
    vi.mocked(api.getMessageBody).mockResolvedValue(
      body({ attachments: [], html: BIG_ATTACH_HTML }),
    );
    vi.mocked(api.downloadExternalAttachment).mockRejectedValue(
      new Error("这个超大附件已经过期，取不回来了"),
    );
    render(<MessageReader message={MESSAGE} />);
    await screen.findByText("虚拟.wav");
    fireEvent.click(screen.getByRole("button", { name: "下载" }));
    await waitFor(() =>
      expect(screen.getByText(/这个超大附件已经过期/)).toBeTruthy(),
    );
    await waitFor(() => {
      const button = screen.getByRole("button", { name: "已过期" }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
    });
  });
});