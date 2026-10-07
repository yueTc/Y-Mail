//! 写信草稿的“未保存判断 + 本地备份”纯函数。
//!
//! 这里只处理正文纯文本、收件人、抄送、密送、主题、账号编号和附件路径，
//! 不碰密码、授权码、令牌，也不发起任何网络请求。本地备份只走浏览器
//! localStorage，作为自动草稿的兜底。

/** localStorage 里草稿备份的键名。 */
export const COMPOSE_DRAFT_STORAGE_KEY = "ymail.compose-draft.v1";

/** 附件在草稿备份里的最小信息；带编号的是正文内嵌图片。 */
export interface ComposeDraftAttachment {
  path: string;
  filename: string;
  /** 正文内嵌图片的编号；普通附件为空。 */
  contentId?: string;
}

/**
 * 判断“有没有未保存改动”时用的快照。字段全部是值类型，直接深比较即可。
 */
export interface ComposeDraftSnapshot {
  accountId?: number;
  toText: string;
  ccText: string;
  bccText: string;
  subject: string;
  /** 正文 HTML；内嵌图片在这里是 `cid:` 引用（备份里不存 base64）。 */
  bodyHtml: string;
  /** 正文纯文本，给邮件的纯文本分片用。 */
  bodyText: string;
  attachments: ComposeDraftAttachment[];
  signatureOn: boolean;
}

/** 落到 localStorage 的备份结构；只包含允许保存的字段。 */
export interface StoredComposeDraft extends ComposeDraftSnapshot {
  kind: string;
  savedAt: string;
}

const MAX_ATTACHMENTS = 200;
const MAX_TEXT = 2_000_000;

function asString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function normalizeAccountId(value: unknown): number | undefined {
  return typeof value === "number" && Number.isInteger(value) && value > 0 ? value : undefined;
}

function normalizeAttachments(value: unknown): ComposeDraftAttachment[] {
  if (!Array.isArray(value)) return [];
  const result: ComposeDraftAttachment[] = [];
  for (const entry of value) {
    if (result.length >= MAX_ATTACHMENTS) break;
    if (entry === null || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    const path = typeof record.path === "string" ? record.path : "";
    const filename = typeof record.filename === "string" ? record.filename : "";
    if (path.trim() === "" && filename.trim() === "") continue;
    const contentId = typeof record.contentId === "string" ? record.contentId.trim() : "";
    result.push(contentId === "" ? { path, filename } : { path, filename, contentId });
  }
  return result;
}

/** 判断当前编辑内容相对初始种子是否有未保存改动。 */
export function hasUnsavedChanges(
  current: ComposeDraftSnapshot,
  seed: ComposeDraftSnapshot | undefined,
): boolean {
  if (!seed) return false;
  return JSON.stringify(normalizeSnapshot(current)) !== JSON.stringify(normalizeSnapshot(seed));
}

/**
 * 把草稿序列化成可写入 localStorage 的字符串。
 * 只保留允许保存的字段，签名、正文 HTML 等一律不进备份。
 */
export function serializeComposeDraft(
  snapshot: ComposeDraftSnapshot,
  kind: string,
  savedAt: string,
): string {
  const normalized = normalizeSnapshot(snapshot);
  const payload: StoredComposeDraft = {
    ...normalized,
    kind,
    savedAt,
  };
  return JSON.stringify(payload);
}

/**
 * 反序列化草稿。任何字段缺失、类型不对或整体不是对象都返回 null，
 * 由调用方决定具体提示。
 */
export function deserializeComposeDraft(raw: string | null | undefined): StoredComposeDraft | null {
  if (typeof raw !== "string" || raw.trim() === "") return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const record = parsed as Record<string, unknown>;
  if (typeof record.kind !== "string" || record.kind.trim() === "") return null;
  if (typeof record.savedAt !== "string") return null;
  const attachments = normalizeAttachments(record.attachments);
  return {
    kind: record.kind,
    savedAt: record.savedAt,
    accountId: normalizeAccountId(record.accountId),
    toText: asString(record.toText).slice(0, MAX_TEXT),
    ccText: asString(record.ccText).slice(0, MAX_TEXT),
    bccText: asString(record.bccText).slice(0, MAX_TEXT),
    subject: asString(record.subject).slice(0, MAX_TEXT),
    bodyHtml: asString(record.bodyHtml).slice(0, MAX_TEXT),
    bodyText: asString(record.bodyText).slice(0, MAX_TEXT),
    attachments,
    signatureOn: record.signatureOn === true,
  };
}

/** 内部归一化：字段缺失按空值算，附件深拷贝，避免调用方后续改到原数组。 */
function normalizeSnapshot(snapshot: unknown): ComposeDraftSnapshot {
  const record = (snapshot ?? {}) as Record<string, unknown>;
  const accountId = normalizeAccountId(record.accountId);
  return {
    toText: asString(record.toText),
    ccText: asString(record.ccText),
    bccText: asString(record.bccText),
    subject: asString(record.subject),
    bodyHtml: asString(record.bodyHtml),
    bodyText: asString(record.bodyText),
    attachments: normalizeAttachments(record.attachments),
    signatureOn: record.signatureOn === true,
    ...(accountId === undefined ? {} : { accountId }),
  };
}