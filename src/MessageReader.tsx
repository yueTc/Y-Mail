//! 读信窗格（Wave 4）。
//!
//! 安全约定：
//! - 正文一律放进 `sandbox` 且不含 `allow-scripts` 的 iframe，文档再上一条严格 CSP；
//! - 远程图片默认拦截，用户点了「本封放行」才把 http/https 加进图片白名单，且不回写数据库；
//! - 附件只给本地下载按钮；可执行文件用红色警示提醒来源风险；
//! - 正文内容不可信，界面不据此跳转、不执行任何脚本。

import { useCallback, useEffect, useMemo, useState } from "react";

import {
  api,
  describeError,
  type InboxMessage,
  type MessageAttachment,
  type MessageBody,
} from "./api";

/** 深色模式偏好；三档存本地，只记界面偏好，不涉及任何敏感信息。 */
export type ReaderTheme = "auto" | "light" | "dark";

/** 本地存储键：只存「跟随系统 / 浅色 / 深色」。 */
export const READER_THEME_KEY = "em-master.reader-theme";

/** 会被当作危险可执行文件的扩展名。 */
const EXECUTABLE_EXTENSIONS = new Set([
  "exe", "com", "bat", "cmd", "scr", "msi", "ps1", "vbs", "js", "jse",
  "wsf", "wsh", "jar", "lnk", "reg", "hta", "cpl", "dll", "pif",
]);

/** 会被当作危险可执行文件的 MIME 类型片段。 */
const EXECUTABLE_MIME_PATTERNS = [
  "application/x-msdownload",
  "application/x-dosexec",
  "application/x-executable",
  "application/x-msdos-program",
  "application/vnd.microsoft.portable-executable",
  "application/x-sh",
];

/** 读本地存储里的主题偏好；隐私模式下拿不到就用「跟随系统」。 */
function readStoredTheme(): ReaderTheme {
  try {
    const value = window.localStorage.getItem(READER_THEME_KEY);
    if (value === "light" || value === "dark" || value === "auto") return value;
  } catch {
    // 本地存储不可用时忽略，不影响读信。
  }
  return "auto";
}

/** 把纯文本正文转义成 HTML，避免把文本当标签解释。 */
function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** 附件是否是可执行文件；命中扩展名或危险 MIME 都算。 */
export function isExecutableAttachment(attachment: MessageAttachment): boolean {
  const name = attachment.filename.trim().toLowerCase();
  const dot = name.lastIndexOf(".");
  if (dot >= 0 && EXECUTABLE_EXTENSIONS.has(name.slice(dot + 1))) return true;
  const mime = attachment.mimeType.trim().toLowerCase();
  return EXECUTABLE_MIME_PATTERNS.some((pattern) => mime.includes(pattern));
}

/** 把字节数说成大白话。 */
export function formatAttachmentSize(size: number): string {
  if (!Number.isFinite(size) || size < 0) return "未知大小";
  if (size < 1024) return `${size} 字节`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  return `${(size / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

/** 把 UTC 时间说成本地可读时间。 */
function formatReaderTime(iso: string): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString("zh-CN", { hour12: false });
}

/** 附件列表里的文件名；空名给个能认出来的兜底。 */
export function attachmentLabel(attachment: MessageAttachment): string {
  const name = attachment.filename.trim();
  if (name) return name;
  return attachment.isInline ? "内嵌图片" : "未命名附件";
}

/**
 * 拼出 iframe 里的完整文档。
 *
 * CSP 说明：默认什么都不许；图片基础只放 `data:` 与 `cid:`，
 * 用户放行本封后才把 `http:` / `https:` 加进去。
 */
export function buildReaderDocument(
  contentHtml: string,
  options: { allowRemoteImages: boolean; dark: boolean },
): string {
  const imgSources = options.allowRemoteImages ? "data: cid: http: https:" : "data: cid:";
  const csp = [
    "default-src 'none'",
    `img-src ${imgSources}`,
    "style-src 'unsafe-inline'",
    "font-src data:",
    "base-uri 'none'",
    "form-action 'none'",
  ].join("; ");
  const background = options.dark ? "#1b1f2a" : "#ffffff";
  const foreground = options.dark ? "#e6e9f2" : "#1f2430";
  const link = options.dark ? "#8ab4ff" : "#1a5fb4";
  return [
    "<!doctype html>",
    '<html lang="zh-CN">',
    "<head>",
    '<meta charset="utf-8">',
    `<meta http-equiv="Content-Security-Policy" content="${csp}">`,
    '<meta name="referrer" content="no-referrer">',
    "<style>",
    `html,body{margin:0;padding:0;background:${background};color:${foreground};`,
    "font-family:'Segoe UI','Microsoft YaHei',system-ui,sans-serif;font-size:15px;line-height:1.6;}",
    "body{padding:14px 16px;word-break:break-word;}",
    `a{color:${link};}`,
    "img{max-width:100%;height:auto;}",
    "table{max-width:100%;border-collapse:collapse;}",
    "pre{white-space:pre-wrap;}",
    "blockquote{margin:8px 0;padding-left:10px;border-left:3px solid rgba(128,128,128,0.5);}",
    "</style>",
    "</head>",
    `<body>${contentHtml}</body>`,
    "</html>",
  ].join("");
}

/** 读信窗格属性。 */
export interface MessageReaderProps {
  /** 当前选中的邮件；为空时只显示提示。 */
  message?: InboxMessage;
}

/** 右栏读信窗格：正文、远程图片放行提示与附件清单。 */
export default function MessageReader({ message }: MessageReaderProps) {
  const [body, setBody] = useState<MessageBody>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [remoteAllowedFor, setRemoteAllowedFor] = useState<number>();
  const [theme, setTheme] = useState<ReaderTheme>(() => readStoredTheme());
  const [systemDark, setSystemDark] = useState(false);
  const [downloading, setDownloading] = useState<ReadonlySet<number>>(new Set());
  const [downloadedPaths, setDownloadedPaths] = useState<Record<number, string>>({});

  const messageId = message?.id;
  const allowRemote = messageId !== undefined && remoteAllowedFor === messageId;

  /** 跟随系统深色：监听系统主题变化。 */
  useEffect(() => {
    const query = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!query) return;
    const update = () => setSystemDark(query.matches);
    update();
    query.addEventListener?.("change", update);
    return () => query.removeEventListener?.("change", update);
  }, []);

  /** 切一封邮件或切换放行状态时重新取正文。 */
  useEffect(() => {
    if (messageId === undefined) {
      setBody(undefined);
      setError(undefined);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(undefined);
    setActionError(undefined);
    api
      .getMessageBody(messageId, allowRemote)
      .then((value) => {
        if (!cancelled) setBody(value);
      })
      .catch((caught: unknown) => {
        if (!cancelled) setError(describeError(caught));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [messageId, allowRemote]);

  const dark = theme === "dark" || (theme === "auto" && systemDark);

  const changeTheme = useCallback((value: ReaderTheme) => {
    setTheme(value);
    try {
      window.localStorage.setItem(READER_THEME_KEY, value);
    } catch {
      // 本地存储不可用时只影响下次打开，不影响当前阅读。
    }
  }, []);

  const download = useCallback(async (attachment: MessageAttachment) => {
    setDownloading((old) => new Set(old).add(attachment.id));
    setActionError(undefined);
    try {
      const path = await api.downloadAttachment(attachment.id);
      setDownloadedPaths((old) => ({ ...old, [attachment.id]: path }));
      setBody((old) =>
        old
          ? {
              ...old,
              attachments: old.attachments.map((item) =>
                item.id === attachment.id
                  ? { ...item, state: "downloaded" as const, localPath: path }
                  : item,
              ),
            }
          : old,
      );
    } catch (caught) {
      setActionError(describeError(caught));
    } finally {
      setDownloading((old) => {
        const next = new Set(old);
        next.delete(attachment.id);
        return next;
      });
    }
  }, []);

  const contentHtml = useMemo(() => {
    if (!body) return "";
    if (body.html) return body.html;
    if (body.textPlain) return `<pre>${escapeHtml(body.textPlain)}</pre>`;
    return "";
  }, [body]);

  const document_ = useMemo(() => {
    if (!contentHtml) return "";
    return buildReaderDocument(contentHtml, { allowRemoteImages: allowRemote, dark });
  }, [contentHtml, allowRemote, dark]);

  if (!message) {
    return (
      <div className="reader-pane">
        <p className="hint reader-empty">从中间列表选一封邮件，这里显示正文与附件。</p>
      </div>
    );
  }

  const blocked = body?.blockedRemoteImages ?? 0;

  return (
    <div className={dark ? "reader-pane reader-dark" : "reader-pane"}>
      <header className="reader-header">
        <div className="reader-head-top">
          <h3 className="reader-subject">{message.subject || "（无主题）"}</h3>
          <label className="reader-theme">
            深色模式
            <select
              aria-label="读信深色模式"
              value={theme}
              onChange={(event) => changeTheme(event.target.value as ReaderTheme)}
            >
              <option value="auto">跟随系统</option>
              <option value="light">浅色</option>
              <option value="dark">深色</option>
            </select>
          </label>
        </div>
        <div className="reader-meta">
          <span>{message.fromName.trim() || message.fromAddr}</span>
          {message.fromAddr && message.fromName.trim() !== "" && (
            <span className="reader-meta-addr">{message.fromAddr}</span>
          )}
          <span>{formatReaderTime(message.dateUtc)}</span>
        </div>
      </header>

      <div className="reader-content">
        {loading && <p className="hint">正在读取正文……</p>}
        {error && <p className="error">读信失败：{error}</p>}
        {actionError && <p className="error">附件操作失败：{actionError}</p>}

        {!loading && !error && body && (
          <>
            {blocked > 0 && !allowRemote && (
              <div className="reader-blocked">
                <span>
                  已拦截远程图片（{blocked} 张）。放行后服务器可能知道你打开了这封邮件，请先确认发件人可信。
                </span>
                <button
                  type="button"
                  className="reader-allow"
                  onClick={() => setRemoteAllowedFor(message.id)}
                >
                  本封放行远程图片
                </button>
              </div>
            )}
            {blocked > 0 && allowRemote && (
              <p className="hint">本封已放行远程图片，关闭后自动恢复默认拦截。</p>
            )}

            {document_ ? (
              <iframe
                className="reader-frame"
                title="邮件正文"
                sandbox=""
                referrerPolicy="no-referrer"
                srcDoc={document_}
              />
            ) : (
              <p className="hint">这封邮件没有可显示的正文。</p>
            )}

            <section className="reader-attachments">
              <h4>附件（{body.attachments.length}）</h4>
              {body.attachments.length === 0 && <p className="hint">没有附件。</p>}
              <ul className="attachment-list">
                {body.attachments.map((attachment) => {
                  const executable = isExecutableAttachment(attachment);
                  const path = downloadedPaths[attachment.id] ?? attachment.localPath ?? undefined;
                  const busy = downloading.has(attachment.id);
                  return (
                    <li
                      key={attachment.id}
                      className={executable ? "attachment-item attachment-danger" : "attachment-item"}
                    >
                      <div className="attachment-info">
                        <span className="attachment-name">{attachmentLabel(attachment)}</span>
                        <span className="attachment-meta">
                          {attachment.mimeType || "未知类型"} · {formatAttachmentSize(attachment.size)}
                          {attachment.isInline ? " · 内嵌" : ""}
                        </span>
                        {executable && (
                          <span className="attachment-warning">
                            可执行文件，打开前请确认来源可信
                          </span>
                        )}
                        {path && <span className="attachment-path">已保存：{path}</span>}
                      </div>
                      <button
                        type="button"
                        className="attachment-download"
                        disabled={busy}
                        onClick={() => void download(attachment)}
                      >
                        {busy ? "下载中……" : path ? "重新下载" : "下载"}
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          </>
        )}
      </div>
    </div>
  );
}