//! 内嵌图片（cid:）前端处理：把正文里的 cid 引用换成受控的本地地址。
//!
//! 安全约定：
//! - 只认识后端给的本地图片；绝不据此发起任何网络请求；
//! - 只接受 `data:image/...;base64,...` 形状的本地地址，其余一律换成占位；
//! - 占位文本用 textContent 写入，不拼字符串，避免注入；
//! - 远程图片（data-em-original-src）原样不动，仍由既有放行开关控制。

import type { InlineImage } from "./api";
import { t } from "./i18n";

/** 与后端一致的单张内嵌图片上限（2 MiB）。 */
export const MAX_INLINE_IMAGE_BYTES = 2 * 1024 * 1024;

/** 受控 data URL 的形状；只允许光栅图，SVG 不在内。 */
const DATA_URL_PATTERN = /^data:image\/(png|jpeg|gif|webp|bmp|avif);base64,[A-Za-z0-9+/=]+$/;

/** 后端 Content-ID 字符白名单：字母数字与 @ . _ - + = ~。 */
const CID_PATTERN = /^[a-z0-9@._+=~-]+$/i;

const MAX_CID_LENGTH = 255;

/**
 * 规范化 Content-ID，口径与后端一致：去尖括号、去空白、转小写、做安全校验。
 * 不合法时返回 undefined。
 */
export function normalizeCid(raw: string | null | undefined): string | undefined {
  if (typeof raw !== "string") return undefined;
  let value = raw.trim();
  if (value.startsWith("<")) value = value.slice(1);
  if (value.endsWith(">")) value = value.slice(0, -1);
  const collapsed = value.replace(/\s+/g, "");
  if (!collapsed || collapsed.length > MAX_CID_LENGTH) return undefined;
  if (collapsed.includes("..") || collapsed.includes("/") || collapsed.includes("\\")) return undefined;
  if (collapsed.startsWith(".") || collapsed.endsWith(".")) return undefined;
  if (!CID_PATTERN.test(collapsed)) return undefined;
  return collapsed.toLowerCase();
}

/** 正文引用可能做了百分号编码，匹配时再试一次解码后的形式。 */
function cidCandidates(raw: string): string[] {
  const candidates: string[] = [];
  const direct = normalizeCid(raw);
  if (direct) candidates.push(direct);
  if (raw.includes("%")) {
    try {
      const decoded = normalizeCid(decodeURIComponent(raw));
      if (decoded && !candidates.includes(decoded)) candidates.push(decoded);
    } catch {
      // 解码失败就当没有，按缺失占位处理。
    }
  }
  return candidates;
}

/** 一张内嵌图片最终有没有内联渲染成功。 */
export interface InlineImageApplication {
  /** 替换 cid 引用之后的正文 HTML。 */
  html: string;
  /** 正文引用了、但本地还没下载的内嵌图片（可点按钮加载）。 */
  pending: InlineImage[];
  /** 成功内联渲染的张数。 */
  rendered: number;
  /** 引用存在但没能渲染的张数（缺失 / 类型不支持 / 过大）。 */
  rejected: number;
}

/**
 * 就地把正文里的 `cid:` 图片引用替换成受控 data URL 或静态占位。
 *
 * 用不会加载资源的惰性 DOM 解析，避免引入新权限；不可用时原样返回。
 */
export function applyInlineImages(contentHtml: string, images: readonly InlineImage[]): InlineImageApplication {
  const result: InlineImageApplication = { html: contentHtml, pending: [], rendered: 0, rejected: 0 };
  if (!contentHtml || typeof DOMParser === "undefined") return result;

  const byCid = new Map<string, InlineImage>();
  for (const image of images) {
    const normalized = normalizeCid(image.contentId);
    if (normalized && !byCid.has(normalized)) byCid.set(normalized, image);
  }

  const document_ = new DOMParser().parseFromString(contentHtml, "text/html");
  const pendingIds = new Set<number>();
  for (const img of Array.from(document_.querySelectorAll("img"))) {
    const raw = img.getAttribute("src");
    if (!raw || !/^cid:/i.test(raw.trim())) continue;
    const reference = raw.trim().slice(4);
    const candidates = cidCandidates(reference);
    const image = candidates.map((candidate) => byCid.get(candidate)).find((found) => found !== undefined);

    if (image && image.state === "available" && image.dataUrl && DATA_URL_PATTERN.test(image.dataUrl)) {
      img.setAttribute("src", image.dataUrl);
      result.rendered += 1;
      continue;
    }

    const placeholder = document_.createElement("span");
    placeholder.className = "em-inline-placeholder";
    placeholder.textContent = placeholderText(image);
    img.replaceWith(placeholder);

    if (image && image.state === "not-downloaded" && image.attachmentId !== null) {
      if (!pendingIds.has(image.attachmentId)) {
        pendingIds.add(image.attachmentId);
        result.pending.push(image);
      }
    } else {
      result.rejected += 1;
    }
  }

  result.html = document_.body ? document_.body.innerHTML : contentHtml;
  return result;
}

/** 占位文字；带原因方便用户判断，不带任何原始地址。 */
function placeholderText(image: InlineImage | undefined): string {
  if (!image) return t("［内嵌图片缺失，未显示］");
  switch (image.state) {
    case "too-large":
      return t("［内嵌图片过大，未显示］");
    case "unsupported":
      return t("［内嵌图片类型不支持，未显示］");
    case "not-downloaded":
      return t("［内嵌图片未加载，点下方按钮加载］");
    case "available":
      return t("［内嵌图片数据异常，未显示］");
    default:
      return t("［内嵌图片未显示］");
  }
}

/** 这张内嵌图是否能安全内联（给界面按钮做二次确认）。 */
export function isInlineImageRenderable(image: InlineImage): boolean {
  return (
    image.state === "available" &&
    typeof image.dataUrl === "string" &&
    DATA_URL_PATTERN.test(image.dataUrl)
  );
}
