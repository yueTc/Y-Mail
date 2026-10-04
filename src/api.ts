//! 前端调用 Tauri 命令的类型化封装。
//!
//! 约定：授权码与代理密码只在这个文件里作为函数参数传递，不落任何本地存储，
//! 界面出参里也永远拿不回已保存的密码本体（只有 hasCredential / hasPassword 布尔值）。

import { invoke } from "@tauri-apps/api/core";

// ============================ 类型 ============================

/** 认证方式：password 是授权码 / 密码，oauth2 是浏览器授权。 */
export type AuthType = "password" | "oauth2";

/** OAuth2 服务商。 */
export type OAuthProvider = "gmail" | "microsoft";

/** 传输加密方式。 */
export type Security = "tls" | "starttls" | "plain";

/** 账号级代理策略。 */
export type AccountProxyMode = "inherit" | "direct" | "custom";

/** 全局代理策略。 */
export type GlobalProxyMode = "system" | "direct" | "custom";

/** 代理类型。 */
export type ProxyKind = "socks5" | "http";

/** 一台服务器的地址、端口与加密方式。 */
export interface ServerConfig {
  host: string;
  port: number;
  security: Security;
}

/** 账号级代理选择。 */
export interface AccountProxy {
  mode: AccountProxyMode;
  proxyId?: number;
}

/** 新建 / 修改账号时提交的草稿；不含授权码。 */
export interface AccountDraft {
  displayName: string;
  email: string;
  authType: AuthType;
  username: string;
  imap: ServerConfig;
  smtp: ServerConfig;
  proxy: AccountProxy;
  color: string;
  enabled: boolean;
  /** OAuth2 服务商；密码登录不传。 */
  oauthProvider?: OAuthProvider;
  /** OAuth2 客户端编号；密码登录不传。 */
  oauthClientId?: string;
}

/** 已保存的账号；不含凭据本体。 */
export interface Account {
  id: number;
  displayName: string;
  email: string;
  authType: AuthType;
  username: string;
  imap: ServerConfig;
  smtp: ServerConfig;
  proxy: AccountProxy;
  color: string;
  enabled: boolean;
  /** OAuth2 服务商；密码登录为 null。 */
  oauthProvider: OAuthProvider | null;
  /** OAuth2 客户端编号；密码登录为空串。 */
  oauthClientId: string;
  hasCredential: boolean;
  createdAt: string;
  updatedAt: string;
}

/** 一次待完成的 OAuth2 授权。 */
export interface OAuthAuthorization {
  /** 已经在系统浏览器里打开的授权页地址。 */
  authorizeUrl: string;
  /** 本次授权的校验串；收口与取消都要带上它。 */
  state: string;
  /** 本机回调地址。 */
  redirectUri: string;
}

/** 授权完成后的账号与自检结果。 */
export interface OAuthOutcome {
  account: Account;
  report: ConnectionReport;
}

/** 一个 OAuth2 账号当前的授权状态。 */
export interface OAuthStatus {
  /** 保险箱里有没有可用的访问令牌。 */
  authorized: boolean;
  /** 到期时间（Unix 秒）；服务器没给就是 null。 */
  expiresAt: number | null;
  /** 申请的权限范围。 */
  scope: string | null;
  /** 有没有刷新令牌。 */
  hasRefreshToken: boolean;
}

/** 新建 / 修改代理时提交的配置；不含密码。 */
export interface ProxyConfig {
  id?: number;
  label: string;
  kind: ProxyKind;
  host: string;
  port: number;
  username: string;
}

/** 已保存的代理；不含密码本体。 */
export interface Proxy {
  id: number;
  label: string;
  kind: ProxyKind;
  host: string;
  port: number;
  username: string;
  hasPassword: boolean;
}

/** 全局代理设置。 */
export interface GlobalProxy {
  mode: GlobalProxyMode;
  proxyId?: number;
}

/** 同步状态标识。 */
export type SyncState =
  | "idle"
  | "connecting"
  | "syncing"
  | "backfilling"
  | "idle_waiting"
  | "error"
  | "needs_reauth"
  | "stopped";

/** 一个账号的同步状态快照。 */
export interface SyncStatus {
  accountId: number;
  email: string;
  state: SyncState;
  stateLabel: string;
  progress: number;
  total: number;
  message: string;
  needsReauth: boolean;
  updatedAt: string;
}
/** 连接自检结果。 */
export interface ConnectionReport {
  imapFolderCount: number;
  smtpMechanism: string;
}

/** 数据库状态快照。 */
export interface DbStatus {
  databaseFile: string;
  logDir: string;
  schemaVersion: number;
  appliedCount: number;
  appliedVersions: number[];
  fts5Available: boolean;
}

// ============================ 统一收件箱类型（Wave 3） ============================

/** 统一收件箱里的一封邮件。 */
export interface InboxMessage {
  id: number;
  accountId: number;
  folderId: number;
  uid: number;
  threadKey: string;
  subject: string;
  fromName: string;
  fromAddr: string;
  dateUtc: string;
  size: number;
  hasAttachments: boolean;
  isRead: boolean;
  isFlagged: boolean;
  snippet: string;
  accountEmail: string;
  accountName: string;
  accountColor: string;
  folderPath: string;
}

/** 折叠后的一条会话线程。 */
export interface InboxThread {
  accountId: number;
  threadKey: string;
  messageCount: number;
  unreadCount: number;
  latest: InboxMessage;
}

/** 一个账号的收件箱汇总。 */
export interface AccountInboxSummary {
  accountId: number;
  email: string;
  displayName: string;
  color: string;
  enabled: boolean;
  messageCount: number;
  unreadCount: number;
}

/** 一个账号下的文件夹（带本地邮件条数）。 */
export interface InboxFolder {
  accountId: number;
  folderId: number;
  fullPath: string;
  kind: string;
  messageCount: number;
  unreadCount: number;
}
/** 收件箱总览：各账号汇总 + 合计。 */
export interface InboxSummary {
  accounts: AccountInboxSummary[];
  totalUnread: number;
  totalMessages: number;
}

/** 收件箱查询条件；不带条件时看全部账号的收件箱。 */
export interface InboxQuery {
  accountId?: number;
  folderId?: number;
  unreadOnly?: boolean;
  offset?: number;
  limit?: number;
}

/** 一页邮件（含总数）。 */
export interface InboxMessagePage {
  items: InboxMessage[];
  total: number;
  offset: number;
  limit: number;
}

/** 一页线程（含总数）。 */
export interface InboxThreadPage {
  items: InboxThread[];
  total: number;
  offset: number;
  limit: number;
}
// ============================ 读信与附件类型（Wave 4） ============================

/** 附件的本地保存状态。 */
export type AttachmentState = "pending" | "downloading" | "downloaded" | "failed";

/** 一个附件的元数据与本地保存状态。 */
export interface MessageAttachment {
  id: number;
  messageId: number;
  partIndex: number;
  filename: string;
  mimeType: string;
  size: number;
  contentId: string | null;
  isInline: boolean;
  localPath: string | null;
  state: AttachmentState;
}

/** 读信窗格要展示的一封邮件。 */
export interface MessageBody {
  messageId: number;
  textPlain: string | null;
  /** 清洗后的 HTML；只有用户放行本封时才带远程图片地址。 */
  html: string | null;
  /** 被拦下的远程图片数量。 */
  blockedRemoteImages: number;
  attachments: MessageAttachment[];
}
// ============================ 搜索与写信类型（Wave 5） ============================

/** 写信类型：新写、回复、转发。 */
export type OutboxKind = "new" | "reply" | "forward";

/** 发件队列里一条记录的状态。 */
export type OutboxState = "draft" | "queued" | "sending" | "sent" | "failed";

/** 搜索条件；`deep` 为真时先联网补一批未同步的历史。 */
export interface SearchQuery {
  raw: string;
  accountId?: number;
  offset?: number;
  limit?: number;
  deep?: boolean;
}

/** 一段高亮片段；`highlighted` 为真表示命中关键词。 */
export interface SnippetSegment {
  text: string;
  highlighted: boolean;
}

/** 一条搜索结果。 */
export interface SearchHit {
  message: InboxMessage;
  snippet: SnippetSegment[];
}

/** 一页搜索结果，附带深拉情况。 */
export interface SearchPage {
  items: SearchHit[];
  total: number;
  offset: number;
  limit: number;
  deepSynced: boolean;
  deepError: string | null;
}

/** 一位收件人。 */
export interface ComposeParticipant {
  name: string;
  address: string;
}

/** 一个待发附件：本地路径 + 展示文件名。 */
export interface ComposeAttachment {
  path: string;
  filename: string;
}

/** 写信窗格的预填内容（新建走空模板，回复 / 转发由外壳组装）。 */
export interface ComposeDraft {
  kind: OutboxKind;
  accountId: number;
  to: ComposeParticipant[];
  cc: ComposeParticipant[];
  bcc: ComposeParticipant[];
  subject: string;
  bodyHtml: string;
  bodyText: string;
  inReplyTo: string | null;
  references: string[];
  attachments: ComposeAttachment[];
}

/** 提交保存的草稿；`id` 省略表示新建。 */
export interface OutboxDraft {
  id?: number;
  accountId: number;
  kind: OutboxKind;
  to: ComposeParticipant[];
  cc: ComposeParticipant[];
  bcc: ComposeParticipant[];
  subject: string;
  bodyHtml: string;
  bodyText: string;
  inReplyTo: string | null;
  references: string[];
  attachments: ComposeAttachment[];
}

/** 发件队列里的一条记录（含账号展示信息）。 */
export interface OutboxItem extends OutboxDraft {
  id: number;
  state: OutboxState;
  attempts: number;
  lastError: string | null;
  createdAt: string;
  updatedAt: string;
  sentAt: string | null;
  accountEmail: string;
  accountDisplayName: string;
}

/** 一封成功投递的报告。 */
export interface SendReport {
  acceptedRecipients: number;
}

/** 跑一轮发送队列的结果。 */
export interface SendOutcome {
  attempted: number;
  sent: number;
  errors: string[];
  reports: SendReport[];
}

/** 一位联系人。 */
export interface Contact {
  id: number;
  accountId: number | null;
  name: string;
  email: string;
  lastUsedAt: string | null;
}

/** 一个账号的签名。 */
export interface Signature {
  accountId: number;
  html: string;
  enabled: boolean;
  updatedAt: string;
}
// ============================ 错误 ============================

/** 命令错误：对 Rust 侧 `CommandError` 的还原。 */
export class ApiError extends Error {
  readonly kind?: string;
  readonly hint?: string;
  readonly details: string[];

  constructor(message: string, kind?: string, hint?: string, details: string[] = []) {
    super(message);
    this.name = "ApiError";
    this.kind = kind;
    this.hint = hint;
    this.details = details;
  }
}

/** 把任意 catch 到的值整理成可读文本。 */
export function describeError(error: unknown): string {
  if (error instanceof ApiError) {
    const parts: string[] = [];
    parts.push(error.kind ? `${error.kind}：${error.message}` : error.message);
    parts.push(...error.details);
    if (error.hint) parts.push(`建议：${error.hint}`);
    return parts.join("；");
  }
  if (error instanceof Error) return error.message;
  return String(error);
}

function normalizeError(error: unknown): ApiError {
  if (error instanceof ApiError) return error;
  if (typeof error === "object" && error !== null) {
    const record = error as Record<string, unknown>;
    const message =
      typeof record.message === "string" && record.message.trim() !== ""
        ? record.message
        : "操作失败，请稍后重试";
    const kind = typeof record.kind === "string" ? record.kind : undefined;
    const hint = typeof record.hint === "string" ? record.hint : undefined;
    const details = Array.isArray(record.details)
      ? record.details.filter((item): item is string => typeof item === "string")
      : [];
    return new ApiError(message, kind, hint, details);
  }
  return new ApiError(String(error));
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw normalizeError(error);
  }
}

// ============================ 命令封装 ============================

export const api = {
  dbStatus: () => call<DbStatus>("db_status"),

  listAccounts: () => call<Account[]>("list_accounts"),

  testAccountConnection: (draft: AccountDraft, secret: string) =>
    call<ConnectionReport>("test_account_connection", { draft, secret }),

  createAccount: (draft: AccountDraft, secret: string) =>
    call<Account>("create_account", { draft, secret }),

  updateAccount: (id: number, draft: AccountDraft, secret?: string) =>
    call<Account>("update_account", secret === undefined ? { id, draft } : { id, draft, secret }),

  deleteAccount: (id: number) => call<void>("delete_account", { id }),

  testSavedAccount: (id: number) => call<ConnectionReport>("test_saved_account", { id }),

  // ===== OAuth2 浏览器授权（Wave 6） =====

  beginOAuthAuthorize: (draft: AccountDraft, accountId?: number) =>
    call<OAuthAuthorization>(
      "begin_oauth_authorize",
      accountId === undefined ? { draft } : { draft, accountId },
    ),

  completeOAuthAuthorize: (stateKey: string) =>
    call<OAuthOutcome>("complete_oauth_authorize", { stateKey }),

  cancelOAuthAuthorize: (stateKey: string) => call<boolean>("cancel_oauth_authorize", { stateKey }),

  oauthStatus: (id: number) => call<OAuthStatus>("oauth_status", { id }),

  listProxies: () => call<Proxy[]>("list_proxies"),

  saveProxy: (config: ProxyConfig, password?: string) =>
    call<Proxy>("save_proxy", password === undefined ? { config } : { config, password }),

  deleteProxy: (id: number) => call<void>("delete_proxy", { id }),

  getProxySettings: () => call<GlobalProxy>("get_proxy_settings"),

  setProxySettings: (mode: GlobalProxy) => call<GlobalProxy>("set_proxy_settings", { mode }),

  syncStatus: () => call<SyncStatus[]>("sync_status"),

  startSync: (accountId?: number) =>
    call<number>("start_sync", accountId === undefined ? {} : { accountId }),

  stopSync: (accountId?: number) =>
    call<void>("stop_sync", accountId === undefined ? {} : { accountId }),
  testProxy: (id: number, target?: string) =>
    call<void>("test_proxy", target === undefined ? { id } : { id, target }),

  // ===== 统一收件箱（Wave 3） =====

  inboxSummary: () => call<InboxSummary>("inbox_summary"),

  listInboxFolders: () => call<InboxFolder[]>("list_inbox_folders"),

  listInboxMessages: (query: InboxQuery = {}) =>
    call<InboxMessagePage>("list_inbox_messages", { query }),

  listInboxThreads: (query: InboxQuery = {}) =>
    call<InboxThreadPage>("list_inbox_threads", { query }),

  listThreadMessages: (accountId: number, threadKey: string, limit?: number) =>
    call<InboxMessage[]>(
      "list_thread_messages",
      limit === undefined ? { accountId, threadKey } : { accountId, threadKey, limit },
    ),

  // ===== 读信与附件（Wave 4） =====

  getMessageBody: (messageId: number, allowRemoteImages = false) =>
    call<MessageBody>("get_message_body", { messageId, allowRemoteImages }),

  downloadAttachment: (attachmentId: number) =>
    call<string>("download_attachment", { attachmentId }),

  // ===== 搜索与写信（Wave 5） =====

  searchMessages: (query: SearchQuery) => call<SearchPage>("search_messages", { query }),

  composeDraft: (kind: OutboxKind, sourceMessageId: number) =>
    call<ComposeDraft>("compose_draft", { kind, sourceMessageId }),

  saveDraft: (draft: OutboxDraft) => call<number>("save_draft", { draft }),

  enqueueOutbox: (id: number) => call<boolean>("enqueue_outbox", { id }),

  retryOutbox: (id: number) => call<boolean>("retry_outbox", { id }),

  listOutbox: (accountId?: number, limit?: number) => {
    const args: Record<string, unknown> = {};
    if (accountId !== undefined) args.accountId = accountId;
    if (limit !== undefined) args.limit = limit;
    return call<OutboxItem[]>("list_outbox", args);
  },

  getOutbox: (id: number) => call<OutboxItem | null>("get_outbox", { id }),

  deleteOutbox: (id: number) => call<boolean>("delete_outbox", { id }),

  searchContacts: (accountId: number, keyword: string, limit?: number) =>
    call<Contact[]>(
      "search_contacts",
      limit === undefined ? { accountId, keyword } : { accountId, keyword, limit },
    ),

  getSignature: (accountId: number) => call<Signature>("get_signature", { accountId }),

  saveSignature: (accountId: number, html: string, enabled: boolean) =>
    call<Signature>("save_signature", { accountId, html, enabled }),

  sendOutbox: () => call<SendOutcome>("send_outbox"),
};