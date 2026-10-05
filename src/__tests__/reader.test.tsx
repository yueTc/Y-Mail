//! 读信窗格前端回归：iframe 沙箱、远程图片放行与附件警示。
//!
//! 这里渲染真实的 `MessageReader`，只把 Tauri 命令层（../api）换成内存桩。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import MessageReader, { buildReaderDocument, isExecutableAttachment } from "../MessageReader";
import type { InboxMessage, MessageAttachment, MessageBody } from "../api";
import { api } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getMessageBody: vi.fn(),
    downloadAttachment: vi.fn(),
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
  it("iframe 带 sandbox 且不包含 allow-scripts", async () => {
    render(<MessageReader message={MESSAGE} />);
    await screen.findByText(/已拦截远程图片/);

    const frame = await screen.findByTitle("邮件正文");
    expect(frame.tagName.toLowerCase()).toBe("iframe");
    const sandbox = frame.getAttribute("sandbox");
    expect(sandbox).not.toBeNull();
    expect(sandbox ?? "").not.toContain("allow-scripts");
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

  it("深色模式选择会记进本地存储", async () => {
    render(<MessageReader message={MESSAGE} />);
    const select = await screen.findByLabelText("读信深色模式");
    fireEvent.change(select, { target: { value: "dark" } });
    expect(window.localStorage.getItem("em-master.reader-theme")).toBe("dark");
  });
});