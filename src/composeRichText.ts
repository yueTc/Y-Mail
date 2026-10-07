//! 写信富文本的纯函数：编辑器里显示的 HTML 与要存 / 要发的 HTML 之间的转换。
//!
//! 编辑器里图片用 data URL 显示（网页读不了本地文件），但存草稿和发信时必须换成
//! `cid:` 引用 + 附件清单里的内嵌图片条目；否则整张 base64 会塞进正文，邮件会大到发不出去。

import { basename } from "./composeAttachments";

/** 正文里的一张内嵌图片（发送用）。 */
export interface InlineImageRef {
  /** 本地文件路径；真正发信时由邮件侧按它读字节。 */
  path: string;
  /** 展示文件名。 */
  filename: string;
  /** 正文 `cid:` 引用的编号。 */
  contentId: string;
}

/** 已经读出 data URL 的内嵌图片（显示用）。 */
export interface ResolvedInlineImage extends InlineImageRef {
  dataUrl: string;
}

/** 图片节点上记本地路径的属性名。 */
export const IMAGE_PATH_ATTR = "data-local-path";
/** 图片节点上记内嵌编号的属性名。 */
export const IMAGE_CID_ATTR = "data-content-id";

/**
 * 生成一个内嵌图片编号。
 * 全小写、只含安全字符，和邮件侧的规范化结果保持一致，避免大小写对不上。
 */
export function newContentId(): string {
  const stamp = Date.now().toString(36);
  const random = Math.random().toString(36).slice(2, 8);
  return `img-${stamp}-${random}@ymail`;
}

/** 解析 HTML；没有 DOM 环境（纯函数测试之外）时返回 null。 */
function parse(html: string): Document | null {
  if (typeof DOMParser === "undefined") return null;
  return new DOMParser().parseFromString(html || "", "text/html");
}

/**
 * 编辑器 HTML → 要存 / 要发的 HTML。
 *
 * 带本地路径的图片换成 `cid:` 引用，路径与编号属性顺手摘掉；
 * 没有本地路径的图片（比如外面贴进来的网图）原样保留。
 */
export function htmlForSending(html: string): { html: string; images: InlineImageRef[] } {
  const document_ = parse(html);
  if (!document_) return { html, images: [] };
  const images: InlineImageRef[] = [];
  const seen = new Set<string>();
  for (const image of Array.from(document_.querySelectorAll("img"))) {
    const path = (image.getAttribute(IMAGE_PATH_ATTR) ?? "").trim();
    const contentId = (image.getAttribute(IMAGE_CID_ATTR) ?? "").trim().toLowerCase();
    if (path === "" || contentId === "") continue;
    image.setAttribute("src", `cid:${contentId}`);
    image.removeAttribute(IMAGE_PATH_ATTR);
    image.removeAttribute(IMAGE_CID_ATTR);
    if (seen.has(contentId)) continue;
    seen.add(contentId);
    const title = (image.getAttribute("title") ?? "").trim();
    images.push({ path, filename: title || basename(path) || "图片", contentId });
  }
  return { html: document_.body.innerHTML, images };
}

/**
 * 要存 / 要发的 HTML → 编辑器里显示的 HTML。
 *
 * `cid:` 引用换成 data URL，并把本地路径与编号写回属性，
 * 这样再次保存时还能还原成内嵌图片。认不出来的编号原样留着。
 */
export function htmlForEditor(html: string, images: readonly ResolvedInlineImage[]): string {
  const document_ = parse(html);
  if (!document_) return html;
  if (images.length === 0) return document_.body.innerHTML;
  const byId = new Map(images.map((item) => [item.contentId.toLowerCase(), item]));
  for (const image of Array.from(document_.querySelectorAll("img"))) {
    const src = (image.getAttribute("src") ?? "").trim();
    if (!src.toLowerCase().startsWith("cid:")) continue;
    const id = src.slice(4).trim().toLowerCase();
    const found = byId.get(id);
    if (!found) continue;
    image.setAttribute("src", found.dataUrl);
    image.setAttribute(IMAGE_PATH_ATTR, found.path);
    image.setAttribute(IMAGE_CID_ATTR, found.contentId);
    if (!image.getAttribute("title")) image.setAttribute("title", found.filename);
    if (!image.getAttribute("alt")) image.setAttribute("alt", found.filename);
  }
  return document_.body.innerHTML;
}

/** 正文里的纯文本（给邮件的纯文本分片用）。 */
export function htmlToText(html: string): string {
  const document_ = parse(html);
  if (!document_) return "";
  return (document_.body.textContent ?? "").trim();
}