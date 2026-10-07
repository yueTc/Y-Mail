//! 读信窗格（Wave 4）。
//!
//! 安全约定：
//! - 正文一律放进不含 `allow-scripts` 的 iframe，文档再上一条严格 CSP；
//!   沙箱额外带 `allow-same-origin`，外层才拿得到正文文档、把链接接管给系统浏览器；
//! - 远程图片默认拦截；用户可「本封放行」，也可记住发件人以后自动放行（名单存本地设置）；
//! - 附件只给本地下载按钮；可执行文件用红色警示提醒来源风险；
//! - 正文内容不可信，界面不据此跳转、不执行任何脚本。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

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
import FlagButton from "./FlagButton";
import ReaderResizer from "./ReaderResizer";
import { isDarkTheme, useReaderTheme, useSystemDark } from "./readerTheme";
import {
  findExternalAttachments,
  isExternalExpired,
  writeClipboard,
  type ExternalAttachment,
} from "./externalAttachments";
import { applyInlineImages, MAX_INLINE_IMAGE_BYTES } from "./inlineImages";
import { useAttachmentHeight, useElementHeight } from "./useAttachmentHeight";

// 深色模式偏好搬到了设置页，这里只保留原有导出，实际读写由 readerTheme.ts 负责。
export { READER_THEME_KEY, type ReaderTheme } from "./readerTheme";

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
  options: {
    allowRemoteImages: boolean;
    dark: boolean;
    translations?: readonly TranslationAnnotation[];
    /** insert：译文逐段插在原文后面；replace：把原文各段文字换成译文。 */
    translationMode?: "insert" | "replace";
  },
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
    `<body>${applyTranslations(contentHtml, options.translations ?? [], options.translationMode ?? "insert")}</body>`,
    "</html>",
  ].join("");
}

/**
 * 正文里哪些链接该交给系统浏览器：只认绝对的 `http` / `https` / `mailto`。
 *
 * 其余一律不外开：`#` 锚点留在正文里跳，相对地址没法外开，
 * `file:`、`javascript:` 和第三方自定义协议都可能被系统协议处理器拿去干别的。
 * 后端还会再校验一次。大小写不敏感。
 */
export function externalReaderLink(href: string | null | undefined): string | undefined {
  const value = (href ?? "").trim();
  if (!/^(https?:\/\/|mailto:)\S/i.test(value)) return undefined;
  return value;
}

/**
 * 给一段正文文档挂上链接接管：点正文里的链接交给系统浏览器，不让正文自己导航。
 *
 * 外层能拿到正文文档，是因为正文 iframe 带了 `allow-same-origin`
 * （依旧没有 `allow-scripts`，正文自己跑不了脚本）。
 * 返回解绑函数，正文整篇换掉时换绑用。
 */
export function bindReaderLinks(doc: Document, onOpen: (url: string) => void): () => void {
  const onClick = (event: Event) => {
    const target = event.target as Element | null;
    const anchor = target?.closest?.("a[href]") ?? null;
    if (!anchor) return;
    const raw = anchor.getAttribute("href") ?? "";
    // 页内锚点仍由 iframe 自己跳，不交给系统浏览器。
    if (raw.startsWith("#")) return;
    // 其余链接一律拦下来：沙箱里正文自己导航既没用，也容易把读信界面顶坏。
    event.preventDefault();
    const url = externalReaderLink(raw);
    if (url) onOpen(url);
  };
  doc.addEventListener("click", onClick, true);
  return () => doc.removeEventListener("click", onClick, true);
}

/**
 * 读信正文 iframe：静态 HTML + 严格 CSP，正文自己不能跑脚本。
 *
 * 沙箱里那条 `allow-same-origin` 是给外层用的：只有同源，外层才拿得到
 * `contentDocument`，把正文里的链接点击接管成「用系统默认浏览器打开」。
 * 没有 `allow-scripts`，正文里的脚本照样跑不起来。
 */
function ReaderFrame({ html, onOpenLink }: { html: string; onOpenLink: (url: string) => void }) {
  const frameRef = useRef<HTMLIFrameElement | null>(null);
  const unbind = useRef<(() => void) | null>(null);

  const bind = useCallback(() => {
    const doc = frameRef.current?.contentDocument;
    if (!doc) return;
    unbind.current?.();
    unbind.current = bindReaderLinks(doc, onOpenLink);
  }, [onOpenLink]);

  useEffect(() => {
    bind();
    return () => {
      unbind.current?.();
      unbind.current = null;
    };
  }, [bind, html]);

  return (
    <iframe
      ref={frameRef}
      className="reader-frame reader-frame-primary"
      title="邮件正文"
      sandbox="allow-same-origin"
      referrerPolicy="no-referrer"
      srcDoc={html}
      onLoad={bind}
    />
  );
}

/** 一段「原文 → 译文」对照，用在正文里就地插译。 */
export interface TranslationAnnotation {
  original: string;
  translated: string;
}

/** 参与读信切段的块级标签，与后端分段规则保持一致。 */
const READER_BLOCK_TAGS = new Set([
  "p", "div", "li", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6",
  "blockquote", "td", "th", "tr", "table", "pre", "section", "article",
  "header", "footer", "main", "dl", "dt", "dd",
]);

/** 这些标签里的文字不算正文，避免把样式表或脚本当成段落。 */
const READER_SKIP_TAGS = new Set(["style", "script", "head", "title", "template"]);

/**
 * 找出「一段」对应的元素，规则贴合后端 mail-ai 的 split_html：
 * - 最外层的块级元素算一段；
 * - <li> 例外，每个列表项各自成段，译文才能逐条落在对应条目后面；
 * - <table> 整块算一段，免得译文被塞进表格行里、把排版顶坏。
 */
function normalizeReaderText(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

/**
 * 比对用的键：把所有空白都去掉。
 * 前后端对 <br>、&nbsp;、换行缩进的还原口径不完全一样，去掉空白再比就不会因为这些差异漏配。
 */
function matchReaderText(value: string): string {
  return value.replace(/\s+/g, "");
}

/**
 * 找一段原文对应的段落元素：
 * 1) 先看游标指向的下一段，命中率最高；
 * 2) 再往后按文本找（前端切段可能比后端多出一段，比如只有图片的段落）；
 * 3) 实在对不上就按顺序兜底，宁可错位也不把译文整段丢到末尾。
 */
function locateReaderSegment(segments: Element[], cursor: number, needle: string): number {
  if (cursor >= segments.length) return -1;
  if (matchReaderText(segments[cursor].textContent ?? "").includes(needle)) return cursor;
  for (let offset = cursor + 1; offset < segments.length; offset += 1) {
    if (matchReaderText(segments[offset].textContent ?? "").includes(needle)) return offset;
  }
  return cursor;
}

/** 整块成段、不再往里钻的块级标签。 */
const READER_LEAF_BLOCK_TAGS = new Set(["table"]);

/** 直接子元素里有没有块级元素；有的话当前元素只当容器。 */
function hasReaderBlockChild(element: Element): boolean {
  for (const child of Array.from(element.children)) {
    const tag = child.tagName.toLowerCase();
    if (READER_SKIP_TAGS.has(tag)) continue;
    if (READER_BLOCK_TAGS.has(tag)) return true;
  }
  return false;
}

/**
 * 深度优先收集「成段」的元素，顺序即原文顺序。
 *
 * `looseText` 决定容器里的裸文字要不要单独成段。进 `<li>` 时关掉：列表项本身已经算了一段，
 * 再把它里面的文字切出来会重复计价。顶层和普通容器打开，这样没有 `<p>` 的邮件也能逐句翻译。
 */
function collectReaderBlocks(node: Node, out: Element[], looseText: boolean): void {
  for (const child of Array.from(node.childNodes)) {
    if (child.nodeType === 3) {
      if (!looseText) continue;
      // 容器里的裸文字也算一段：先包一层行内 <span>，再当成段落处理，排版不变。
      const textNode = child as Text;
      if (normalizeReaderText(textNode.nodeValue ?? "")) {
        const owner = textNode.ownerDocument ?? document;
        const span = owner.createElement("span");
        textNode.replaceWith(span);
        span.appendChild(textNode);
        out.push(span);
      }
      continue;
    }
    if (child.nodeType !== 1) continue;
    const element = child as Element;
    const tag = element.tagName.toLowerCase();
    if (READER_SKIP_TAGS.has(tag)) continue;
    if (!READER_BLOCK_TAGS.has(tag)) {
      // 行内元素不单独成段，继续往里找块级元素。
      collectReaderBlocks(element, out, looseText);
      continue;
    }
    if (tag === "li" || READER_LEAF_BLOCK_TAGS.has(tag)) {
      if (normalizeReaderText(element.textContent ?? "")) out.push(element);
      // 列表项里的嵌套列表还要继续往下找，但不再拆列表项自己的文字。
      if (tag === "li") collectReaderBlocks(element, out, false);
      continue;
    }
    if (hasReaderBlockChild(element)) {
      // 里面还有块级元素，它只当容器，继续往里找最内层的段落。
      collectReaderBlocks(element, out, looseText);
      continue;
    }
    if (normalizeReaderText(element.textContent ?? "")) out.push(element);
  }
}

function walkReaderBlocks(root: Node, out: Element[]): void {
  collectReaderBlocks(root, out, true);
}

/** 译文块：只放纯文字，样式内联，避免邮件自带样式把它吃掉。 */
function createTranslationElement(owner: Document, translated: string): HTMLElement {
  const box = owner.createElement("div");
  box.className = "em-inline-translation";
  box.setAttribute("data-em-translation", "1");
  box.setAttribute(
    "style",
    "margin:8px 0 4px;padding:6px 10px;border-left:3px solid #2f7cf6;border-radius:4px;background:rgba(47,124,246,0.08);white-space:pre-wrap;word-break:break-word;",
  );
  // textContent 不解析 HTML，模型就算吐出标签也只会被当普通文字。
  box.textContent = translated;
  return box;
}

/** 把译文放到目标元素后面；表格与列表里没有 div 的位置，改用合法的子元素。 */
function insertTranslationElement(target: Element, translated: string): void {
  const box = createTranslationElement(target.ownerDocument, translated);
  const tag = target.tagName.toLowerCase();
  const owner = target.ownerDocument;
  if (tag === "tr") {
    const cell = owner.createElement("td");
    cell.setAttribute("colspan", "99");
    cell.appendChild(box);
    target.appendChild(cell);
    return;
  }
  if (tag === "ul" || tag === "ol") {
    const item = owner.createElement("li");
    item.appendChild(box);
    target.appendChild(item);
    return;
  }
  if (tag === "li") {
    // 列表项要插成兄弟 <li>，直接塞 <div> 会被列表当成非法子元素。
    const item = owner.createElement("li");
    item.appendChild(box);
    target.insertAdjacentElement("afterend", item);
    return;
  }
  target.insertAdjacentElement("afterend", box);
}

/**
 * 把译文按段落嵌回邮件正文：先按后端同样的块级规则切段，再逐段插在原文后面。
 * 这样图片、链接、原始排版都留在原位，译文只多出一块，不会把正文变成纯文字。
 */
export function annotateEmailTranslations(
  contentHtml: string,
  translations: readonly TranslationAnnotation[],
): string {
  if (!contentHtml || translations.length === 0) return contentHtml;
  if (typeof document === "undefined") return contentHtml;
  // 单独建一个不挂到页面上的文档，解析邮件 HTML 时不会执行脚本，也不影响当前界面。
  const parsed = document.implementation.createHTMLDocument("");
  parsed.body.innerHTML = contentHtml;
  const segments: Element[] = [];
  walkReaderBlocks(parsed.body, segments);
  let cursor = 0;
  const leftovers: string[] = [];
  for (const item of translations) {
    const needle = matchReaderText(item.original);
    if (!needle) continue;
    const translated = item.translated ?? "";
    const target = locateReaderSegment(segments, cursor, needle);
    if (target >= 0) {
      cursor = target + 1;
      if (translated.trim()) insertTranslationElement(segments[target], translated);
    } else if (translated.trim()) {
      // 段落用完了也不能丢：留到最后统一补在正文末尾。
      leftovers.push(translated);
    }
  }
  if (leftovers.length > 0) {
    const tail = parsed.createElement("section");
    tail.setAttribute("data-em-translation-tail", "1");
    for (const leftover of leftovers) {
      tail.appendChild(createTranslationElement(parsed, leftover));
    }
    parsed.body.appendChild(tail);
  }
  return parsed.body.innerHTML;
}

/** 块里嵌了子列表 / 表格时不替换文字，改成「原文 + 译文」并列，免得把内层结构弄丢。 */
function blockHasNestedBlock(element: Element): boolean {
  return element.querySelector("ul,ol,table") !== null;
}

/** 递归收集一块里全部文字节点，顺序即原文顺序（链接里的文字也算）。 */
function collectTextNodes(node: Node, out: Text[]): void {
  for (const child of Array.from(node.childNodes)) {
    if (child.nodeType === 3) {
      out.push(child as Text);
    } else if (child.nodeType === 1) {
      collectTextNodes(child, out);
    }
  }
}

/**
 * 把一块的文字换成译文：按各文字节点原来的长度比例把译文切开分配。
 * 这样 <a>、<strong>、<img> 这些元素都留在原位，链接照样能点，
 * 链接里的文字也一起变成中文。
 */
function replaceBlockText(element: Element, translated: string): void {
  const nodes: Text[] = [];
  collectTextNodes(element, nodes);
  if (nodes.length === 0) {
    element.textContent = translated;
    return;
  }
  const lengths = nodes.map((node) => (node.nodeValue ?? "").length);
  const total = lengths.reduce((sum, length) => sum + length, 0);
  if (total === 0) {
    nodes[0].nodeValue = translated;
    return;
  }
  // 按原文字数比例分配译文的字数。
  const counts = lengths.map((length) => Math.round((length / total) * translated.length));
  const nonEmpty = lengths.filter((length) => length > 0).length;
  let sum = counts.reduce((acc, count) => acc + count, 0);
  if (translated.length >= nonEmpty) {
    // 原文有字的节点至少分到一个字，免得链接里的字被切空、链接看不见。
    counts.forEach((count, index) => {
      if (lengths[index] > 0 && count === 0) {
        counts[index] = 1;
        sum += 1;
      }
    });
  }
  // 多退少补，保证合计正好等于译文长度。
  while (sum > translated.length) {
    let largest = -1;
    counts.forEach((count, index) => {
      if (count > 1 && (largest === -1 || count > counts[largest])) largest = index;
    });
    if (largest === -1) break;
    counts[largest] -= 1;
    sum -= 1;
  }
  if (sum < translated.length && counts.length > 0) {
    let largest = 0;
    counts.forEach((count, index) => {
      if (count > counts[largest]) largest = index;
    });
    counts[largest] += translated.length - sum;
  }
  let cursor = 0;
  nodes.forEach((node, index) => {
    node.nodeValue = translated.slice(cursor, cursor + counts[index]);
    cursor += counts[index];
  });
}

/**
 * 直接翻译 / 对照右栏用的文档：保留邮件原有的标签，只把每段文字换成译文。
 * 标题还是标题、列表还是列表、链接和图片留在原位，文字全部换成中文。
 */
export function replaceEmailTranslations(
  contentHtml: string,
  translations: readonly TranslationAnnotation[],
): string {
  if (!contentHtml || translations.length === 0) return contentHtml;
  if (typeof document === "undefined") return contentHtml;
  const parsed = document.implementation.createHTMLDocument("");
  parsed.body.innerHTML = contentHtml;
  const segments: Element[] = [];
  walkReaderBlocks(parsed.body, segments);
  let cursor = 0;
  const leftovers: string[] = [];
  for (const item of translations) {
    const needle = matchReaderText(item.original);
    if (!needle) continue;
    const translated = (item.translated ?? "").trim();
    if (!translated) continue;
    const index = locateReaderSegment(segments, cursor, needle);
    if (index < 0) {
      // 段落用完了也不能丢：留到最后统一补在正文末尾。
      leftovers.push(translated);
      continue;
    }
    cursor = index + 1;
    const target = segments[index];
    if (blockHasNestedBlock(target)) {
      insertTranslationElement(target, translated);
    } else {
      replaceBlockText(target, translated);
    }
  }
  for (const leftover of leftovers) {
    const paragraph = parsed.createElement("p");
    paragraph.textContent = leftover;
    parsed.body.appendChild(paragraph);
  }
  return parsed.body.innerHTML;
}

/** 按模式选插入还是替换；没有译文时原样返回。 */
function applyTranslations(
  contentHtml: string,
  translations: readonly TranslationAnnotation[],
  mode: "insert" | "replace",
): string {
  if (translations.length === 0) return contentHtml;
  return mode === "replace"
    ? replaceEmailTranslations(contentHtml, translations)
    : annotateEmailTranslations(contentHtml, translations);
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
  /** 切换红旗；不传时标题区不显示按钮。 */
  onToggleFlag?: (message: InboxMessage) => void;
}

/** 右栏读信窗格：正文、远程图片放行提示与附件清单。 */
export default function MessageReader({
  message,
  aiEnabled = false,
  onToggleFlag,
}: MessageReaderProps) {
  const [body, setBody] = useState<MessageBody>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [linkError, setLinkError] = useState<string>();
  const [remoteAllowedFor, setRemoteAllowedFor] = useState<number>();
  /** 本封已记住发件人（界面提示用）。 */
  const [rememberedFor, setRememberedFor] = useState<number>();
  const [rememberBusy, setRememberBusy] = useState(false);
  const [theme] = useReaderTheme();
  const systemDark = useSystemDark();
  const [downloading, setDownloading] = useState<ReadonlySet<number>>(new Set());
  const [downloadedPaths, setDownloadedPaths] = useState<Record<number, string>>({});
  const [inlineBusy, setInlineBusy] = useState<ReadonlySet<number>>(new Set());
  const [reloadKey, setReloadKey] = useState(0);
  /** 刚复制过链接的外部大附件地址（界面提示用）。 */
  const [copiedExternalLink, setCopiedExternalLink] = useState<string>();
  /** 正在下载的外部大附件地址。 */
  const [externalBusy, setExternalBusy] = useState<ReadonlySet<string>>(new Set());
  /** 已经下回来的外部大附件：地址 → 本地路径。 */
  const [externalSaved, setExternalSaved] = useState<Record<string, string>>({});
  /** 服务器说过期的外部大附件地址（正文里的到期时间也可能已经过期）。 */
  const [externalExpired, setExternalExpired] = useState<ReadonlySet<string>>(new Set());
  /** 正在用系统程序打开的本地文件。 */
  const [openingFile, setOpeningFile] = useState<string>();
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

  const dark = isDarkTheme(theme, systemDark);

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

  /** 下载正文里的外部大附件（网易超大附件）；过期由后端或正文里的到期时间给出。 */
  const downloadExternal = useCallback(async (item: ExternalAttachment) => {
    setExternalBusy((old) => new Set(old).add(item.href));
    setActionError(undefined);
    try {
      const saved = await api.downloadExternalAttachment(item.href);
      setExternalSaved((old) => ({ ...old, [item.href]: saved.path }));
    } catch (caught) {
      const message = describeError(caught);
      setActionError(message);
      if (message.includes("过期")) {
        setExternalExpired((old) => new Set(old).add(item.href));
      }
    } finally {
      setExternalBusy((old) => {
        const next = new Set(old);
        next.delete(item.href);
        return next;
      });
    }
  }, []);

  /** 用系统默认程序打开一个已经下载好的附件。 */
  const openSavedFile = useCallback(async (path: string) => {
    setOpeningFile(path);
    setActionError(undefined);
    try {
      await api.openDownloadedFile(path);
    } catch (caught) {
      setActionError(describeError(caught));
    } finally {
      setOpeningFile(undefined);
    }
  }, []);

  /** 打开附件所在的位置。 */
  const openSavedFileDir = useCallback(async (path: string) => {
    setOpeningFile(path);
    setActionError(undefined);
    try {
      await api.openDownloadedFileDir(path);
    } catch (caught) {
      setActionError(describeError(caught));
    } finally {
      setOpeningFile(undefined);
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

  /** 复制外部大附件下载链接；失败时提示手动选中。 */
  const copyExternalLink = useCallback(async (href: string) => {
    setActionError(undefined);
    const ok = await writeClipboard(href);
    if (!ok) {
      setActionError("复制失败，请手动选中链接复制。");
      return;
    }
    setCopiedExternalLink(href);
  }, []);

  /** 点正文里的链接：交给系统默认浏览器打开，别让正文自己跳走。 */
  const openExternalLink = useCallback(async (url: string) => {
    setLinkError(undefined);
    try {
      await api.openExternalUrl(url);
    } catch (caught) {
      setLinkError(describeError(caught));
    }
  }, []);
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

  /** 原邮件的完整 HTML 文档：对照模式左栏、切回原文都用它，正文原样渲染。 */
  const document_ = useMemo(() => {
    if (!inlineApplication.html) return "";
    return buildReaderDocument(inlineApplication.html, { allowRemoteImages: allowRemote, dark });
  }, [inlineApplication.html, allowRemote, dark]);

  /** 行内翻译用的文档：原邮件照旧，译文逐段插在对应内容后面。 */
  const inlineDocument_ = useMemo(() => {
    if (!inlineApplication.html || !translation) return "";
    return buildReaderDocument(inlineApplication.html, {
      allowRemoteImages: allowRemote,
      dark,
      translations: translation.original.map((original, index) => ({
        original,
        translated: translation.translated[index] ?? "",
      })),
    });
  }, [inlineApplication.html, allowRemote, dark, translation]);

  /** 对照翻译右栏 / 直接翻译用的文档：原邮件排版原样，每段文字换成译文。 */
  const translatedDocument_ = useMemo(() => {
    if (!inlineApplication.html || !translation) return "";
    return buildReaderDocument(inlineApplication.html, {
      allowRemoteImages: allowRemote,
      dark,
      translations: translation.original.map((original, index) => ({
        original,
        translated: translation.translated[index] ?? "",
      })),
      translationMode: "replace",
    });
  }, [inlineApplication.html, allowRemote, dark, translation]);

  const { ref: readerBodyRef, height: readerBodyHeight } = useElementHeight<HTMLDivElement>();
  const {
    height: attachmentHeight,
    min: attachmentMinHeight,
    max: attachmentMaxHeight,
    setHeight: setAttachmentHeight,
    commit: commitAttachmentHeight,
    reset: resetAttachmentHeight,
  } = useAttachmentHeight(readerBodyHeight);
  const externalAttachments = useMemo(
    () => findExternalAttachments(body?.html ?? ""),
    [body?.html],
  );
  const hasAttachments = (body?.attachments.length ?? 0) > 0;
  const hasExternalAttachments = externalAttachments.length > 0;
  // 附件区为空时整块不显示，也不出拖动条。
  const showAttachmentPane = hasAttachments || hasExternalAttachments;

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
          {onToggleFlag && (
            <FlagButton
              flagged={message.isFlagged}
              onToggle={() => onToggleFlag(message)}
              label={message.isFlagged ? "取消标红这封邮件" : "标红这封邮件"}
              className="reader-flag"
            />
          )}
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
            aria-busy={translationBusy}
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
                aria-pressed={translationMode === "side_by_side"}
                onClick={() => setTranslationMode("side_by_side")}
              >
                对照翻译
              </button>
              <button
                type="button"
                className={translationMode === "inline" ? "active" : ""}
                aria-pressed={translationMode === "inline"}
                onClick={() => setTranslationMode("inline")}
              >
                行内翻译
              </button>
              <button
                type="button"
                className={translationMode === "direct" ? "active" : ""}
                aria-pressed={translationMode === "direct"}
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
            aria-busy={summaryBusy}
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
          <p className="reader-ai-note" role="status">该模型不支持所选思考程度，已按默认调用。</p>
        )}
        {translationError && <p className="error" role="alert">翻译失败：{translationError}</p>}
        {summaryError && <p className="error" role="alert">摘要失败：{summaryError}</p>}
      </header>

      <div className="reader-body" ref={readerBodyRef}>
        <div className="reader-content" aria-busy={loading}>
        {loading && <p className="hint" role="status">正在读取正文……</p>}
        {error && <p className="error" role="alert">读信失败：{error}</p>}
        {actionError && <p className="error" role="alert">附件操作失败：{actionError}</p>}
        {linkError && <p className="error" role="alert">打开链接失败：{linkError}</p>}

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
                    aria-busy={rememberBusy}
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
              <p className="hint" role="status">
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
                              aria-busy={busy}
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
                  <p className="hint" role="status">
                    还有 {inlineApplication.rejected} 张内嵌图片未显示（缺失、类型不支持或超过{" "}
                    {formatAttachmentSize(MAX_INLINE_IMAGE_BYTES)}）。
                  </p>
                )}
              </section>
            )}

            {translation && translationMode === "direct" ? (
              translatedDocument_ ? (
                <ReaderFrame
                  html={translatedDocument_}
                  onOpenLink={openExternalLink}
                />
              ) : (
                <p className="hint">这封邮件没有可显示的正文。</p>
              )
            ) : translation && translationMode === "side_by_side" ? (
              <div className="reader-translation-columns">
                <div className="reader-translation-column">
                  {document_ && (
                    <ReaderFrame html={document_} onOpenLink={openExternalLink} />
                  )}
                </div>
                <div className="reader-translation-column">
                  {translatedDocument_ && (
                    <ReaderFrame
                      html={translatedDocument_}
                      onOpenLink={openExternalLink}
                    />
                  )}
                </div>
              </div>
            ) : (
              <>
                {(translationMode === "inline" ? inlineDocument_ : document_) && (
                  <ReaderFrame
                    html={translationMode === "inline" ? inlineDocument_ : document_}
                    onOpenLink={openExternalLink}
                  />
                )}
                {!(translationMode === "inline" ? inlineDocument_ : document_) && (
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

          </>
        )}
        </div>

        {showAttachmentPane && (
          <ReaderResizer
            label="附件区高度"
            value={attachmentHeight}
            min={attachmentMinHeight}
            max={attachmentMaxHeight}
            onChange={setAttachmentHeight}
            onCommit={commitAttachmentHeight}
            onReset={resetAttachmentHeight}
          />
        )}

        {!loading && !error && body && showAttachmentPane && (
          <section
            className="reader-attachments reader-attachments-docked"
            aria-label="附件"
            style={{ height: attachmentHeight }}
          >
            {hasAttachments && (
              <>
                <h4>附件（{body.attachments.length}）</h4>
                <ul className="attachment-list">
                  {body.attachments.map((attachment) => {
                    const executable = isExecutableAttachment(attachment);
                    const path =
                      downloadedPaths[attachment.id] ?? attachment.localPath ?? undefined;
                    const busy = downloading.has(attachment.id);
                    return (
                      <li
                        key={attachment.id}
                        className={
                          executable ? "attachment-item attachment-danger" : "attachment-item"
                        }
                      >
                        <div className="attachment-info">
                          <span className="attachment-name">{attachmentLabel(attachment)}</span>
                          <span className="attachment-meta">
                            {attachment.mimeType || "未知类型"} ·{" "}
                            {formatAttachmentSize(attachment.size)}
                            {attachment.isInline ? " · 内嵌" : ""}
                          </span>
                          {executable && (
                            <span className="attachment-warning">
                              可执行文件，打开前请确认来源可信
                            </span>
                          )}
                          {path && <span className="attachment-path">已保存：{path}</span>}
                        </div>
                        <span className="attachment-actions">
                          <button
                            type="button"
                            className="attachment-download"
                            aria-busy={busy}
                            disabled={busy}
                            onClick={() => void download(attachment)}
                          >
                            {busy ? "下载中……" : path ? "重新下载" : "下载"}
                          </button>
                          {path && (
                            <>
                              <button
                                type="button"
                                disabled={openingFile === path}
                                onClick={() => void openSavedFile(path)}
                              >
                                打开文件
                              </button>
                              <button
                                type="button"
                                disabled={openingFile === path}
                                onClick={() => void openSavedFileDir(path)}
                              >
                                打开所在位置
                              </button>
                            </>
                          )}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </>
            )}

            {hasExternalAttachments && (
              <>
                <h4>外部大附件（{externalAttachments.length}）</h4>
                <p className="hint">
                  文件放在邮箱服务器上，点「下载」本应用替你取回；下载完可以直接打开。
                </p>
                <ul className="attachment-list">
                  {externalAttachments.map((item) => {
                    const expired =
                      externalExpired.has(item.href) || isExternalExpired(item, new Date());
                    const busy = externalBusy.has(item.href);
                    const path = externalSaved[item.href];
                    return (
                      <li key={item.href} className="attachment-item">
                        <div className="attachment-info">
                          <span className="attachment-name">{item.name}</span>
                          <span className="attachment-meta">
                            {[item.sizeText, item.expiresText].filter(Boolean).join(" · ") ||
                              "存在邮箱服务器上"}
                            {expired && <span className="attachment-expired">已过期</span>}
                          </span>
                          <span className="attachment-path">
                            {path ? `已保存：${path}` : item.href}
                          </span>
                        </div>
                        <span className="attachment-actions">
                          <button
                            type="button"
                            className="attachment-download"
                            aria-busy={busy}
                            disabled={busy || expired}
                            title={
                              expired ? "这个超大附件已经过期，取不回来了" : "从网易服务器下载"
                            }
                            onClick={() => void downloadExternal(item)}
                          >
                            {busy ? "下载中……" : expired ? "已过期" : path ? "重新下载" : "下载"}
                          </button>
                          <button type="button" onClick={() => void copyExternalLink(item.href)}>
                            {copiedExternalLink === item.href ? "已复制" : "复制链接"}
                          </button>
                          {path && (
                            <>
                              <button
                                type="button"
                                disabled={openingFile === path}
                                onClick={() => void openSavedFile(path)}
                              >
                                打开文件
                              </button>
                              <button
                                type="button"
                                disabled={openingFile === path}
                                onClick={() => void openSavedFileDir(path)}
                              >
                                打开所在位置
                              </button>
                            </>
                          )}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </>
            )}
          </section>
        )}
      </div>
    </div>
  );
}
