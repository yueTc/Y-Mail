//! 读信窗格（Wave 4）。
//!
//! 安全约定：
//! - 正文一律放进 `sandbox` 且不含 `allow-scripts` 的 iframe，文档再上一条严格 CSP；
//! - 远程图片默认拦截；用户可「本封放行」，也可记住发件人以后自动放行（名单存本地设置）；
//! - 附件只给本地下载按钮；可执行文件用红色警示提醒来源风险；
//! - 正文内容不可信，界面不据此跳转、不执行任何脚本。

import { useCallback, useEffect, useMemo, useState } from "react";

import {
  api,
  describeError,
  type AiAuthorization,
  type AiTranslation,
  type InboxMessage,
  type InlineImage,
  type MessageAttachment,
  type MessageBody,
} from "./api";
import AiAuthorizationDialog from "./AiAuthorizationDialog";
import { applyInlineImages, MAX_INLINE_IMAGE_BYTES } from "./inlineImages";

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
    "span.em-inline-placeholder{display:inline-block;padding:2px 6px;border:1px dashed rgba(128,128,128,0.6);border-radius:4px;font-size:12px;line-height:1.5;opacity:0.85;}",
    "</style>",
    "</head>",
    `<body>${contentHtml}</body>`,
    "</html>",
  ].join("");
}

/** 翻译三种显示模式。 */
export type TranslationMode = "side_by_side" | "inline" | "direct";

/** 目标语言选项。 */
export const TRANSLATION_LANGUAGES = [
  { value: "zh-CN", label: "简体中文" },
  { value: "zh-TW", label: "繁体中文" },
  { value: "en", label: "英语" },
  { value: "ja", label: "日语" },
  { value: "ko", label: "韩语" },
  { value: "fr", label: "法语" },
  { value: "de", label: "德语" },
  { value: "es", label: "西班牙语" },
] as const;

/** 读信窗格属性。 */
export interface MessageReaderProps {
  /** 当前选中的邮件；为空时只显示提示。 */
  message?: InboxMessage;
  /** 当前有没有启用的 AI 站点；没有时显示「需启用」。 */
  aiEnabled?: boolean;
}

/** 段落对齐译文的纯文本渲染；不注入 HTML，也不会自动跳转链接。 */
function TranslationPair({
  original,
  translated,
  mode,
}: {
  original: string;
  translated: string;
  mode: TranslationMode;
}) {
  if (mode === "side_by_side") {
    return (
      <div className="reader-translation-pair">
        <p className="reader-translation-original">{original}</p>
        <p className="reader-translation-translated">{translated}</p>
      </div>
    );
  }
  return <p className="reader-translation-translated">{translated}</p>;
}

/** 右栏读信窗格：正文、远程图片放行提示与附件清单。 */
export default function MessageReader({ message, aiEnabled = false }: MessageReaderProps) {
  const [body, setBody] = useState<MessageBody>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [remoteAllowedFor, setRemoteAllowedFor] = useState<number>();
  /** 本封已记住发件人（界面提示用）。 */
  const [rememberedFor, setRememberedFor] = useState<number>();
  const [rememberBusy, setRememberBusy] = useState(false);
  const [theme, setTheme] = useState<ReaderTheme>(() => readStoredTheme());
  const [systemDark, setSystemDark] = useState(false);
  const [downloading, setDownloading] = useState<ReadonlySet<number>>(new Set());
  const [downloadedPaths, setDownloadedPaths] = useState<Record<number, string>>({});
  const [inlineBusy, setInlineBusy] = useState<ReadonlySet<number>>(new Set());
  const [reloadKey, setReloadKey] = useState(0);
  const [targetLanguage, setTargetLanguage] = useState("zh-CN");
  const [translationMode, setTranslationMode] = useState<TranslationMode>("side_by_side");
  const [translation, setTranslation] = useState<AiTranslation>();
  const [translationBusy, setTranslationBusy] = useState(false);
  const [translationError, setTranslationError] = useState("");
  const [translationAuth, setTranslationAuth] = useState<AiAuthorization>();
  const [translationAuthBusy, setTranslationAuthBusy] = useState(false);
  const [summaryText, setSummaryText] = useState("");
  const [summaryBusy, setSummaryBusy] = useState(false);
  const [summaryError, setSummaryError] = useState("");
  const [summaryAuth, setSummaryAuth] = useState<AiAuthorization>();
  const [summaryAuthBusy, setSummaryAuthBusy] = useState(false);
  const [aiDowngraded, setAiDowngraded] = useState(false);

  const messageId = message?.id;
  /** 用户本封点过放行（请求参数用它，避免后端已经自动放行时重复请求）。 */
  const userAllowedRemote = messageId !== undefined && remoteAllowedFor === messageId;
  /** 界面是否按放行渲染：用户本封放行，或后端因「记住的发件人」已自动放行。 */
  const allowRemote = userAllowedRemote || body?.remoteImagesAllowed === true;

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
      .getMessageBody(messageId, userAllowedRemote)
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
  }, [messageId, userAllowedRemote, reloadKey]);

  /** 切邮件时清掉上一封的内嵌图加载状态，避免串封。 */
  useEffect(() => {
    setInlineBusy(new Set());
    setRememberedFor(undefined);
    setRememberBusy(false);
  }, [messageId]);

  /** 切邮件时清掉上一封的 AI 结果，避免串封。 */
  useEffect(() => {
    setTranslation(undefined);
    setSummaryText("");
    setTranslationError("");
    setSummaryError("");
    setAiDowngraded(false);
    setTranslationAuth(undefined);
    setSummaryAuth(undefined);
  }, [messageId]);

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
      if (attachment.isInline) setReloadKey((value) => value + 1);
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

  /**
   * 「点一下加载」内嵌图片：复用既有附件下载命令，下载成功后重新取一次正文。
   *
   * 只有用户点击才会联网；渲染正文本身绝不触发下载。
   */
  const loadInlineImage = useCallback(async (image: InlineImage) => {
    if (image.attachmentId === null || image.state !== "not-downloaded") return;
    setInlineBusy((old) => new Set(old).add(image.attachmentId as number));
    setActionError(undefined);
    try {
      await api.downloadAttachment(image.attachmentId);
      setReloadKey((value) => value + 1);
    } catch (caught) {
      setActionError(describeError(caught));
    } finally {
      setInlineBusy((old) => {
        const next = new Set(old);
        if (image.attachmentId !== null) next.delete(image.attachmentId);
        return next;
      });
    }
  }, []);

  /** 记住本封发件人：以后这个发件人的邮件自动放行远程图片。 */
  const rememberSender = useCallback(async () => {
    if (messageId === undefined || rememberBusy) return;
    setRememberBusy(true);
    setActionError(undefined);
    try {
      await api.rememberRemoteSender(messageId);
      setRemoteAllowedFor(messageId);
      setRememberedFor(messageId);
    } catch (caught) {
      setActionError(describeError(caught));
    } finally {
      setRememberBusy(false);
    }
  }, [messageId, rememberBusy]);

  /** 翻译按钮：先拿只读预览；缓存命中不弹窗，直接取结果。 */
  const requestTranslation = useCallback(async () => {
    if (messageId === undefined || translationBusy) return;
    setTranslationBusy(true);
    setTranslationError("");
    try {
      const preview = await api.aiAuthorizationPreview("translate", {
        messageId,
        targetLanguage,
      });
      if (preview.fromCache) {
        const result = await api.translateMessage(messageId, targetLanguage, "");
        setTranslation(result);
        setAiDowngraded(result.thinkingDowngraded);
        return;
      }
      setTranslationAuth(preview);
    } catch (caught) {
      setTranslationError(describeError(caught));
    } finally {
      setTranslationBusy(false);
    }
  }, [messageId, targetLanguage, translationBusy]);

  /** 用户在授权框点了确认后才真正外发。 */
  const confirmTranslation = useCallback(async () => {
    if (!translationAuth || messageId === undefined) return;
    setTranslationAuthBusy(true);
    setTranslationError("");
    try {
      const result = await api.translateMessage(
        messageId,
        targetLanguage,
        translationAuth.authorizationToken,
      );
      setTranslation(result);
      setAiDowngraded(result.thinkingDowngraded);
      setTranslationAuth(undefined);
    } catch (caught) {
      setTranslationError(describeError(caught));
    } finally {
      setTranslationAuthBusy(false);
    }
  }, [messageId, targetLanguage, translationAuth]);

  /** 摘要按钮走同一套预览与授权流程。 */
  const requestSummary = useCallback(async () => {
    if (messageId === undefined || summaryBusy) return;
    setSummaryBusy(true);
    setSummaryError("");
    try {
      const preview = await api.aiAuthorizationPreview("summary", { messageId });
      if (preview.fromCache) {
        const result = await api.summarizeMessage(messageId, "");
        setSummaryText(result.text);
        setAiDowngraded(result.thinkingDowngraded);
        return;
      }
      setSummaryAuth(preview);
    } catch (caught) {
      setSummaryError(describeError(caught));
    } finally {
      setSummaryBusy(false);
    }
  }, [messageId, summaryBusy]);

  const confirmSummary = useCallback(async () => {
    if (!summaryAuth || messageId === undefined) return;
    setSummaryAuthBusy(true);
    setSummaryError("");
    try {
      const result = await api.summarizeMessage(messageId, summaryAuth.authorizationToken);
      setSummaryText(result.text);
      setAiDowngraded(result.thinkingDowngraded);
      setSummaryAuth(undefined);
    } catch (caught) {
      setSummaryError(describeError(caught));
    } finally {
      setSummaryAuthBusy(false);
    }
  }, [messageId, summaryAuth]);

  const rawContentHtml = useMemo(() => {
    if (!body) return "";
    if (body.html) return body.html;
    if (body.textPlain) return `<pre>${escapeHtml(body.textPlain)}</pre>`;
    return "";
  }, [body]);

  /** 把正文里的 cid 引用换成受控 data URL 或静态占位；缺失的图不进 iframe。 */
  const inlineApplication = useMemo(
    () => applyInlineImages(rawContentHtml, body?.inlineImages ?? []),
    [rawContentHtml, body?.inlineImages],
  );

  const document_ = useMemo(() => {
    if (!inlineApplication.html) return "";
    return buildReaderDocument(inlineApplication.html, { allowRemoteImages: allowRemote, dark });
  }, [inlineApplication.html, allowRemote, dark]);

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
      {translationAuth && (
        <AiAuthorizationDialog
          preview={translationAuth}
          busy={translationAuthBusy}
          onCancel={() => setTranslationAuth(undefined)}
          onConfirm={() => void confirmTranslation()}
        />
      )}
      {summaryAuth && (
        <AiAuthorizationDialog
          preview={summaryAuth}
          busy={summaryAuthBusy}
          onCancel={() => setSummaryAuth(undefined)}
          onConfirm={() => void confirmSummary()}
        />
      )}
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
        <div className="reader-ai-tools">
          <button
            type="button"
            disabled={!aiEnabled || translationBusy}
            onClick={() => void requestTranslation()}
            title={aiEnabled ? "翻译这封邮件" : "需先在设置里启用 AI 站点"}
          >
            {translationBusy ? "翻译中……" : aiEnabled ? "翻译" : "翻译（需启用）"}
          </button>
          <label>
            目标语言
            <select
              aria-label="目标语言"
              value={targetLanguage}
              onChange={(event) => {
                setTargetLanguage(event.target.value);
                setTranslation(undefined);
              }}
              disabled={!aiEnabled}
            >
              {TRANSLATION_LANGUAGES.map((language) => (
                <option key={language.value} value={language.value}>
                  {language.label}
                </option>
              ))}
            </select>
          </label>
          {translation && (
            <div className="reader-translation-modes" role="group" aria-label="翻译显示模式">
              <button
                type="button"
                className={translationMode === "side_by_side" ? "active" : ""}
                onClick={() => setTranslationMode("side_by_side")}
              >
                对照翻译
              </button>
              <button
                type="button"
                className={translationMode === "inline" ? "active" : ""}
                onClick={() => setTranslationMode("inline")}
              >
                行内翻译
              </button>
              <button
                type="button"
                className={translationMode === "direct" ? "active" : ""}
                onClick={() => setTranslationMode("direct")}
              >
                直接翻译
              </button>
              <button type="button" onClick={() => setTranslation(undefined)}>
                切回原文
              </button>
            </div>
          )}
          <button
            type="button"
            disabled={!aiEnabled || summaryBusy}
            onClick={() => void requestSummary()}
            title={aiEnabled ? "摘要这封邮件" : "需先在设置里启用 AI 站点"}
          >
            {summaryBusy ? "摘要中……" : aiEnabled ? "摘要" : "摘要（需启用）"}
          </button>
        </div>
        {!aiEnabled && (
          <p className="hint reader-ai-disabled">
            AI 和翻译默认关闭，需到「账号与代理」设置里添加并启用站点。
          </p>
        )}
        {aiDowngraded && (
          <p className="reader-ai-note">该模型不支持所选思考程度，已按默认调用。</p>
        )}
        {translationError && <p className="error">翻译失败：{translationError}</p>}
        {summaryError && <p className="error">摘要失败：{summaryError}</p>}
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
                <span className="reader-blocked-actions">
                  <button
                    type="button"
                    className="reader-allow"
                    onClick={() => setRemoteAllowedFor(message.id)}
                  >
                    本封放行远程图片
                  </button>
                  <button
                    type="button"
                    className="reader-remember"
                    disabled={rememberBusy}
                    title={`记住 ${message.fromAddr}，以后自动显示远程图片`}
                    onClick={() => void rememberSender()}
                  >
                    {rememberBusy ? "记住中……" : "以后这个发件人都自动显示"}
                  </button>
                </span>
              </div>
            )}
            {blocked > 0 && allowRemote && (
              <p className="hint">
                {rememberedFor === message.id
                  ? "已记住这个发件人，以后自动显示远程图片；可在「账号与代理」设置里移除。"
                  : "本封已放行远程图片，关闭后自动恢复默认拦截。"}
              </p>
            )}

            {(inlineApplication.pending.length > 0 || inlineApplication.rejected > 0) && (
              <section className="reader-inline-images" aria-label="内嵌图片">
                {inlineApplication.pending.length > 0 && (
                  <>
                    <p className="hint">
                      有 {inlineApplication.pending.length} 张内嵌图片还没下载。渲染时不会联网，点「点一下加载」才会去邮箱服务器取。
                    </p>
                    <ul className="reader-inline-list">
                      {inlineApplication.pending.map((image) => {
                        const attachment = body?.attachments.find(
                          (item) => item.id === image.attachmentId,
                        );
                        const busy = image.attachmentId !== null && inlineBusy.has(image.attachmentId);
                        return (
                          <li key={image.contentId}>
                            <span className="reader-inline-name">
                              {attachment ? attachmentLabel(attachment) : image.contentId}
                            </span>
                            <button
                              type="button"
                              className="reader-inline-load"
                              disabled={busy}
                              onClick={() => void loadInlineImage(image)}
                            >
                              {busy ? "加载中……" : "点一下加载"}
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  </>
                )}
                {inlineApplication.rejected > 0 && (
                  <p className="hint">
                    还有 {inlineApplication.rejected} 张内嵌图片未显示（缺失、类型不支持或超过{" "}
                    {formatAttachmentSize(MAX_INLINE_IMAGE_BYTES)}）。
                  </p>
                )}
              </section>
            )}

            {translation && translationMode !== "side_by_side" ? (
              <section className="reader-translation" aria-label="段落对齐译文">
                {translation.original.length === 0 && <p className="hint">这封邮件没有可翻译的正文。</p>}
                {translation.original.map((original, index) => (
                  <TranslationPair
                    key={index}
                    original={original}
                    translated={translation.translated[index] ?? ""}
                    mode={translationMode}
                  />
                ))}
              </section>
            ) : (
              <>
                {translation && translationMode === "side_by_side" && (
                  <section className="reader-translation reader-translation-columns" aria-label="段落对齐译文">
                    <div className="reader-translation-column">
                      <h4>原文</h4>
                      {translation.original.map((original, index) => (
                        <p key={index}>{original}</p>
                      ))}
                    </div>
                    <div className="reader-translation-column">
                      <h4>译文</h4>
                      {translation.translated.map((translated, index) => (
                        <p key={index}>{translated}</p>
                      ))}
                    </div>
                  </section>
                )}
                {!translation && document_ && (
                  <iframe
                    className="reader-frame"
                    title="邮件正文"
                    sandbox=""
                    referrerPolicy="no-referrer"
                    srcDoc={document_}
                  />
                )}
                {!translation && !document_ && (
                  <p className="hint">这封邮件没有可显示的正文。</p>
                )}
              </>
            )}

            {summaryText && (
              <section className="reader-summary" aria-label="邮件摘要">
                <h4>摘要</h4>
                <p>{summaryText}</p>
              </section>
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