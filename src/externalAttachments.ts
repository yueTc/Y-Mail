//! 正文里的「外部大附件」识别。
//!
//! 网易 163 / 126 的超大附件不走 MIME 附件位：正文里只有一条指向
//! `large-attachment-download` 的下载链接，文件本体存在网易服务器上。
//! `BODYSTRUCTURE` 看不到它，所以读信页要单独把这类链接列出来。
//!
//! 这里只做识别与展示：不自动打开、不自动下载，正文永远只是数据。

/** 正文里的一条外部大附件。 */
export interface ExternalAttachment {
  /** 下载页地址，原样取自正文，仅供用户复制。 */
  href: string;
  /** 文件名；认不出来时给兜底文案。 */
  name: string;
  /** 形如 `80.96M` 的大小描述；认不出来则空串。 */
  sizeText: string;
  /** 形如 `2026年8月17日 0:49 到期` 的过期描述；认不出来则空串。 */
  expiresText: string;
}

/** 地址里出现这些特征词就当成外部大附件。 */
const HREF_HINTS = ["large-attachment-download"];

/** 链接文字是这些词时，不拿它当文件名。 */
const GENERIC_NAMES = new Set([
  "下载",
  "点击下载",
  "立即下载",
  "高速下载",
  "download",
  "download now",
]);

/** 认不出文件名时的兜底文案。 */
const FALLBACK_NAME = "外部大附件";

/** 括号里的大小只认这种形状，避免把正文别的内容当成大小。 */
const SIZE_PATTERN = /^\d+(?:\.\d+)?\s*[KMGTP]?B?$/i;

/** `(80.96M, 2026年8月17日 0:49 到期)` 这种写法。 */
const SIZE_AND_EXPIRY_PATTERN = /\(([^()，,]{1,24})[,，]\s*([^()]{1,60}到期[^()]{0,10})\)/;

/** 收拾正文里的多余空白。 */
function tidy(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

/** 地址是不是外部大附件。 */
export function isExternalAttachmentHref(href: string): boolean {
  const lower = href.trim().toLowerCase();
  if (!lower.startsWith("http://") && !lower.startsWith("https://")) return false;
  return HREF_HINTS.some((hint) => lower.includes(hint));
}

/** 从一段文字里抠出大小和过期时间。 */
function readSizeAndExpiry(text: string): { sizeText: string; expiresText: string } {
  const matched = text.match(SIZE_AND_EXPIRY_PATTERN);
  if (!matched) return { sizeText: "", expiresText: "" };
  const size = tidy(matched[1]);
  return {
    sizeText: SIZE_PATTERN.test(size) ? size : "",
    expiresText: tidy(matched[2]),
  };
}

/**
 * 挑文件名。
 *
 * 优先用链接文字；链接文字是「下载」这类通用词时，退回去看它所在容器的文字，
 * 取括号前面的那一段（网易的超大附件块里，文件名就写在那里）。
 */
function pickName(anchorText: string, containerText: string): string {
  if (anchorText && !GENERIC_NAMES.has(anchorText.toLowerCase())) return anchorText;
  const beforeParen = tidy(containerText.split(/[(（]/)[0]);
  if (beforeParen && beforeParen.length <= 120 && !GENERIC_NAMES.has(beforeParen.toLowerCase())) {
    return beforeParen;
  }
  return FALLBACK_NAME;
}

/**
 * 扫出正文里的外部大附件。
 *
 * 同一地址出现多次（文件名一处、下载按钮一处）只留一条。
 */
export function findExternalAttachments(html: string): ExternalAttachment[] {
  if (!html.trim() || typeof DOMParser === "undefined") return [];
  const document_ = new DOMParser().parseFromString(html, "text/html");
  const found = new Map<string, ExternalAttachment>();

  for (const anchor of Array.from(document_.querySelectorAll("a[href]"))) {
    const href = (anchor.getAttribute("href") ?? "").trim();
    if (!isExternalAttachmentHref(href) || found.has(href)) continue;

    const anchorText = tidy(anchor.textContent ?? "");
    // 往上找三层，够碰到装着文件名和大小说明的那个块。
    let container = anchor.parentElement;
    let containerText = anchorText;
    for (let depth = 0; depth < 3 && container; depth += 1) {
      const text = tidy(container.textContent ?? "");
      if (text) {
        containerText = text;
        if (SIZE_AND_EXPIRY_PATTERN.test(text)) break;
      }
      container = container.parentElement;
    }

    const { sizeText, expiresText } = readSizeAndExpiry(containerText);
    found.set(href, {
      href,
      name: pickName(anchorText, containerText),
      sizeText,
      expiresText,
    });
  }

  return Array.from(found.values());
}

/** `2026年8月17日 0:49 到期` 这种写法。 */
const EXPIRY_PATTERN = /(\d{4})年(\d{1,2})月(\d{1,2})日(?:\s*(\d{1,2}):(\d{2}))?/;

/**
 * 从正文写的到期时间算出时刻；认不出来返回 null。
 *
 * 时分缺省时按当天 23:59 算，宁可多给用户一点时间，也别提前说人家过期。
 */
export function externalExpiryTime(item: ExternalAttachment): Date | null {
  const matched = item.expiresText.match(EXPIRY_PATTERN);
  if (!matched) return null;
  const year = Number(matched[1]);
  const month = Number(matched[2]);
  const day = Number(matched[3]);
  const hour = matched[4] === undefined ? 23 : Number(matched[4]);
  const minute = matched[5] === undefined ? 59 : Number(matched[5]);
  if (month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59) return null;
  const when = new Date(year, month - 1, day, hour, minute, 0, 0);
  return Number.isNaN(when.getTime()) ? null : when;
}

/** 这个外部大附件是不是已经过期了（只看正文里写的到期时间）。 */
export function isExternalExpired(item: ExternalAttachment, now: Date): boolean {
  const when = externalExpiryTime(item);
  return when !== null && when.getTime() < now.getTime();
}

/**
 * 把文字放进系统剪贴板。
 *
 * 先试标准接口；老内核或非安全上下文里退回到临时文本框 + 复制命令。
 * 返回是否成功。
 */
export async function writeClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 落到下面的兜底方案。
  }

  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "readonly");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(area);
    return ok;
  } catch {
    return false;
  }
}