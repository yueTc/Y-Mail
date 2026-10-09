//! 前端调用 Tauri 命令的类型化封装。
//!
//! 约定：授权码与代理密码只在这个文件里作为函数参数传递，不落任何本地存储，
//! 界面出参里也永远拿不回已保存的密码本体（只有 hasCredential / hasPassword 布尔值）。

import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { t } from "./i18n";

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

/** 存储目录与通知开关快照。 */
export interface AppSettings {
  /** 已保存的邮件数据目录；空字符串表示用默认。 */
  dataDir: string;
  /** 已保存的附件下载目录；空字符串表示用默认。 */
  attachmentDir: string;
  /** 新邮件是否弹系统通知。 */
  notifyNewMail: boolean;
  /** 通知智能识别开关；默认关，开启后新邮件正文会自动发往所选 AI。 */
  notifyAiEnabled: boolean;
  /** 读信是否默认拦截远程图片；出厂与默认都是拦。 */
  blockRemoteImagesByDefault: boolean;
  /** 关闭主窗口时收进托盘还是退出应用；默认收。 */
  minimizeToTrayOnClose: boolean;
  /** 启动时是不是直接进托盘、不弹主窗口；默认否。 */
  startMinimizedToTray: boolean;
  /** 是否自动检测更新；默认关。 */
  autoCheckUpdate: boolean;
  /** 自动检测更新的间隔小时数；默认 24，允许 1–168。 */
  updateCheckIntervalHours: number;
  /** 默认邮件数据目录。 */
  defaultDataDir: string;
  /** 附件目录留空时会用的默认位置。 */
  defaultAttachmentDir: string;
  /** 当前引擎实际在用的邮件数据目录（改过要重启才生效）。 */
  activeDataDir: string;
  /** 当前引擎实际在用的附件目录。 */
  activeAttachmentDir: string;
  /** 是不是全新安装后的第一次启动；界面据此弹「数据放哪」向导。 */
  firstRun: boolean;
}

/** 保存存储目录与通知开关时提交的内容。 */
export interface AppSettingsInput {
  dataDir: string;
  attachmentDir: string;
  notifyNewMail: boolean;
  notifyAiEnabled: boolean;
  blockRemoteImagesByDefault: boolean;
}

/** 更改数据目录的结果；需要确认时先弹一次确认，再带 confirmed 重试。 */
export interface ChangeDataDirResult {
  needsConfirmation: boolean;
  message: string;
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
  /** 只看标红旗的邮件（左侧「红旗邮件」入口用）。旧载荷可能缺省。 */
  flaggedOnly?: boolean;
  /** 只看某一类文件夹：inbox / draft / sent / trash / junk / custom。旧载荷可能缺省。 */
  folderKind?: string;
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

/** 外部大附件（网易超大附件）下载完的结果。 */
export interface ExternalDownload {
  path: string;
  filename: string;
  size: number;
}

/** 读信窗格要展示的一封邮件。 */
export type InlineImageState = "available" | "not-downloaded" | "too-large" | "unsupported";

export interface InlineImage {
  contentId: string;
  attachmentId: number | null;
  mimeType: string;
  size: number;
  state: InlineImageState;
  dataUrl: string | null;
}

export interface MessageBody {
  messageId: number;
  textPlain: string | null;
  /** 清洗后的 HTML；只有用户放行本封时才带远程图片地址。 */
  html: string | null;
  /** 被拦下的远程图片数量。 */
  blockedRemoteImages: number;
  /** 本次是否放行了远程图片（用户本封放行，或发件人在「记住」名单里）。旧载荷可能缺省。 */
  remoteImagesAllowed?: boolean;
  /** 正文可用的内嵌图片（cid → 本地图片）；没缓存的不含图片字节。旧载荷可能缺省。 */
  inlineImages?: InlineImage[];
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

/** 一张可插入正文的图片：本地路径 + 展示用 data URL。 */
export interface InlineImageInfo {
  path: string;
  filename: string;
  mimeType: string;
  dataUrl: string;
  bytes: number;
}

/** 一个待发附件：本地路径 + 展示文件名；带编号的是正文内嵌图片。 */
export interface ComposeAttachment {
  path: string;
  filename: string;
  /** 正文内嵌图片的编号；普通附件为空。 */
  contentId?: string | null;
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

/** 一位联系人。一个邮箱一条。 */
export interface Contact {
  id: number;
  name: string;
  email: string;
  /** 本地备注，纯文本。 */
  note: string;
  groupId: number | null;
  groupName: string | null;
  /** auto = 同步收集；manual = 用户建或改过。 */
  source: "auto" | "manual";
  hidden: boolean;
  lastUsedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

/** 一个新分组。 */
export interface ContactGroup {
  id: number;
  name: string;
  memberCount: number;
}

/** 通讯录条数快照。 */
export interface ContactCounts {
  active: number;
  hidden: number;
  ungrouped: number;
}

/** 新建 / 修改联系人时提交的字段。 */
export interface ContactDraft {
  name: string;
  email: string;
  note: string;
  groupId: number | null;
}

/** 导出结果。 */
export interface ContactExport {
  count: number;
  path: string;
}

/** 导入里读不出来的一行。 */
export interface ContactProblem {
  line: number;
  reason: string;
}

/** 导入里的一条联系人。 */
export interface ContactImportEntry {
  name: string;
  email: string;
  note: string;
  group: string;
}

/** 导入预览。 */
export interface ContactImportPreview {
  headers: string[];
  emailColumn: number | null;
  entries: ContactImportEntry[];
  problems: ContactProblem[];
  duplicateCount: number;
  newCount: number;
}

/** 导入落库结果。 */
export interface ContactImportOutcome {
  imported: number;
  skipped: number;
  overwritten: number;
  invalid: number;
  groupsCreated: number;
}

/** 一个账号的签名。 */
export interface Signature {
  accountId: number;
  html: string;
  enabled: boolean;
  updatedAt: string;
}
// ============================ AI 与翻译类型（Wave 7） ============================

/** AI 站点类型：OpenAI 兼容、DeepL、本机 Ollama。 */
export type AiProviderKind = "openai_compatible" | "deepl" | "ollama";

/** 思考程度四档。 */
export type AiThinkingLevel = "off" | "low" | "medium" | "high";

/** 可以使用 AI 的功能。 */
export type AiFunction = "translate" | "summary" | "polish" | "draft" | "notification_verify";

/** 新建 / 修改 AI 站点时提交的配置；不包含密钥明文。 */
export interface AiProviderDraft {
  id?: number;
  label: string;
  kind: AiProviderKind;
  baseUrl: string;
  defaultModel: string;
  models: string[];
  thinkingLevel: AiThinkingLevel;
  enabled: boolean;
}

/** 已保存的 AI 站点；只回 `hasKey`，不回密钥。 */
export interface AiProvider extends AiProviderDraft {
  id: number;
  hasKey: boolean;
}

/** 功能级模型配置。 */
export interface AiModelMap {
  function: AiFunction;
  providerId: number;
  model: string;
  thinkingLevel: AiThinkingLevel | null;
  updatedAt: string;
}

/** 通知识别「检查延迟」的一次结果。 */
export interface NotificationLatency {
  /** 一次外发往返的毫秒数。 */
  elapsedMs: number;
  /** 有没有认出验证码。 */
  foundCode: boolean;
  /** 有没有认出验证链接。 */
  foundLink: boolean;
}

/** 外发授权弹窗需要展示的目标信息。 */
export interface AiAuthorization {
  function: AiFunction;
  providerId: number;
  providerLabel: string;
  host: string;
  model: string;
  local: boolean;
  contentHash: string;
  authorizationToken: string;
  expiresInSeconds: number;
  fromCache: boolean;
}

/** 摘要 / 润色 / 起草的纯文本结果。 */
export interface AiTextOutcome {
  text: string;
  thinkingDowngraded: boolean;
  fromCache: boolean;
}

/** 段落对齐的翻译结果；三种显示模式共用。 */
export interface AiTranslation {
  original: string[];
  translated: string[];
  thinkingDowngraded: boolean;
  fromCache: boolean;
}

/** 一条 AI 调用审计；不含正文与密钥。 */
export interface AiAudit {
  id: number;
  createdAt: string;
  function: string;
  providerId: number | null;
  providerLabel: string;
  model: string;
  targetHost: string;
  local: boolean;
  outbound: boolean;
  outcome: string;
  detail: string;
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
    parts.push(error.kind ? t("{0}：{1}", [error.kind, error.message]) : error.message);
    parts.push(...error.details);
    if (error.hint) parts.push(t("建议：{0}", [error.hint]));
    return parts.join(t("；"));
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
        : t("操作失败，请稍后重试");
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

// ============================ 应用内更新 ============================

/**
 * 刚查到、还没装的更新句柄。
 *
 * 查和装是两步：`check()` 拿到的句柄在 Rust 侧占着一份资源，
 * 装完或用户重新查之前必须关掉，否则会一直挂着。
 */
let pendingUpdate: Update | null = null;

/** 丢掉上一次查到的更新句柄；关不掉也不影响用户重新查一次。 */
async function releasePendingUpdate(): Promise<void> {
  const previous = pendingUpdate;
  pendingUpdate = null;
  if (!previous) return;
  try {
    await previous.close();
  } catch {
    // 这里只做清理，失败不往外抛。
  }
}

/** 查到的更新信息；只带界面要显示的字段，不带下载句柄。 */
export interface UpdateInfo {
  /** 更新包里的版本号。 */
  version: string;
  /** 当前正在运行的版本号。 */
  currentVersion: string;
  /** 发布说明原文；没有就空字符串。 */
  notes: string;
  /** 发布时间；没有就空字符串。 */
  date: string;
}

// ============================ 命令封装 ============================

export const api = {
  getAppSettings: () => call<AppSettings>("get_app_settings"),

  saveAppSettings: (input: AppSettingsInput) =>
    call<AppSettings>("set_app_settings", { input }),

  changeDataDir: (newDir: string, confirmed: boolean) =>
    call<ChangeDataDirResult>("change_data_dir", { newDir, confirmed }),

  openDataDir: () => call<void>("open_data_dir"),

  restartApp: (cleanup: boolean) => call<void>("restart_app", { cleanup }),

  // ===== 外观：页面缩放 =====

  /** 页面缩放：整块界面一起放大缩小；数值会被外壳再夹一次范围。 */
  setUiZoom: (scale: number) => call<void>("set_ui_zoom", { scale }),

  // ===== 开机启动 =====

  autostartStatus: () => call<boolean>("autostart_status"),

  setAutostart: (enabled: boolean) => call<boolean>("set_autostart", { enabled }),

  // ===== 启动与托盘 =====

  /** 保存「关闭时最小化到托盘」「启动时最小化到托盘」两个开关。 */
  setTraySettings: (minimizeToTrayOnClose: boolean, startMinimizedToTray: boolean) =>
    call<AppSettings>("set_tray_settings", { minimizeToTrayOnClose, startMinimizedToTray }),

  /** 保存自动检测更新设置；只动这两个字段，别的设置不受影响。 */
  setUpdateSettings: (autoCheckUpdate: boolean, intervalHours: number) =>
    call<AppSettings>("set_update_settings", { autoCheckUpdate, intervalHours }),

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
  /** 测试一个代理能不能真的访问到目标；返回往返耗时（毫秒）。 */
  testProxy: (id: number, target?: string) =>
    call<number>("test_proxy", target === undefined ? { id } : { id, target }),

  // ===== 统一收件箱（Wave 3） =====

  inboxSummary: () => call<InboxSummary>("inbox_summary"),

  listInboxFolders: () => call<InboxFolder[]>("list_inbox_folders"),

  listInboxMessages: (query: InboxQuery = {}) =>
    call<InboxMessagePage>("list_inbox_messages", { query }),

  /** 按编号取一封邮件；通知被点击时用来定位并打开那封邮件。 */
  getInboxMessage: (messageId: number) =>
    call<InboxMessage | null>("get_inbox_message", { messageId }),
  listInboxThreads: (query: InboxQuery = {}) =>
    call<InboxThreadPage>("list_inbox_threads", { query }),

  listThreadMessages: (accountId: number, threadKey: string, limit?: number) =>
    call<InboxMessage[]>(
      "list_thread_messages",
      limit === undefined ? { accountId, threadKey } : { accountId, threadKey, limit },
    ),

  /** 切换一封邮件的已读状态；本地立即生效并刷新未读数。 */
  setMessageRead: (messageId: number, read: boolean) =>
    call<void>("set_message_read", { messageId, read }),

  /** 切换一封邮件的红旗状态；本地立即生效，返回服务器是否已确认。 */
  setMessageFlagged: (messageId: number, flagged: boolean) =>
    call<{ changed: boolean; synced: boolean }>("set_message_flagged", { messageId, flagged }),
  // ===== 读信与附件（Wave 4） =====

  getMessageBody: (messageId: number, allowRemoteImages = false) =>
    call<MessageBody>("get_message_body", { messageId, allowRemoteImages }),

  downloadAttachment: (attachmentId: number) =>
    call<string>("download_attachment", { attachmentId }),

  /** 下载正文里的外部大附件（网易超大附件）；链接由后端再校验一次域名。 */
  downloadExternalAttachment: (url: string) =>
    call<ExternalDownload>("download_external_attachment", { url }),

  /** 用系统默认程序打开已经下载好的附件；路径必须落在下载目录里。 */
  openDownloadedFile: (path: string) => call<void>("open_downloaded_file", { path }),

  /** 打开附件所在的位置，并尽量把文件选中。 */
  openDownloadedFileDir: (path: string) => call<void>("open_downloaded_file_dir", { path }),

  /** 用系统默认浏览器打开正文里的外部链接；后端只放行 http / https / mailto。 */
  openExternalUrl: (url: string) => call<void>("open_external_url", { url }),

  // ===== 写信配图（截图 / 插入图片） =====

  /** 读本地图片并校验；返回展示用 data URL 与发送时要用的路径。 */
  readInlineImage: (path: string) => call<InlineImageInfo>("read_inline_image", { path }),

  /** 存粘贴进来的图片（base64），返回路径与 data URL。 */
  saveInlineImage: (dataBase64: string) =>
    call<InlineImageInfo>("save_inline_image", { dataBase64 }),

  /** 打开全屏截图：主窗口会先藏起来，截完自动回来。 */
  openScreenshotOverlay: () => call<void>("open_screenshot_overlay"),

  /** 取截屏预览（整屏冻结图），给截图窗当背景。 */
  takeScreenshotPreview: () => call<string>("take_screenshot_preview"),

  /** 按框选的坐标裁下截图并落盘；结果通过事件发回主窗口。坐标是物理像素。 */
  finishScreenshot: (x: number, y: number, width: number, height: number) =>
    call<void>("finish_screenshot", { x, y, width, height }),

  /** 取消截图并把主窗口叫回来。 */
  cancelScreenshot: () => call<void>("cancel_screenshot"),

  /** 记住这封邮件的发件人：以后这个发件人的邮件自动放行远程图片。 */
  rememberRemoteSender: (messageId: number) =>
    call<string[]>("remember_remote_sender", { messageId }),

  /** 当前记住的发件人名单。 */
  listTrustedRemoteSenders: () => call<string[]>("list_trusted_remote_senders"),

  /** 移除一个记住的发件人；移除后恢复默认拦截。 */
  forgetRemoteSender: (address: string) => call<string[]>("forget_remote_sender", { address }),

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

  /** 收件人补全：只搜没被隐藏的人。 */
  searchContacts: (keyword: string, limit?: number) =>
    call<Contact[]>(
      "search_contacts",
      limit === undefined ? { keyword } : { keyword, limit },
    ),

  /** 通讯录列表；`scope` 传 `hidden` 看「已隐藏」。 */
  listContacts: (keyword?: string, limit?: number, scope?: "active" | "hidden") =>
    call<Contact[]>("list_contacts", {
      ...(keyword === undefined ? {} : { keyword }),
      ...(limit === undefined ? {} : { limit }),
      ...(scope === undefined ? {} : { scope }),
    }),

  /** 通讯录条数快照。 */
  contactCounts: () => call<ContactCounts>("contact_counts"),

  /** 新建联系人，返回新编号。 */
  createContact: (draft: ContactDraft) => call<number>("create_contact", { draft }),

  /** 修改联系人。 */
  updateContact: (id: number, draft: ContactDraft) => call<void>("update_contact", { id, draft }),

  /** 隐藏一位联系人（软删）。 */
  hideContact: (id: number) => call<void>("hide_contact", { id }),

  /** 把一位已隐藏的联系人放回来。 */
  restoreContact: (id: number) => call<void>("restore_contact", { id }),

  /** 彻底删掉一位联系人。 */
  purgeContact: (id: number) => call<void>("purge_contact", { id }),

  /** 列出全部分组。 */
  listContactGroups: () => call<ContactGroup[]>("list_contact_groups"),

  /** 新建分组，返回新编号。 */
  createContactGroup: (name: string) => call<number>("create_contact_group", { name }),

  /** 给分组改名。 */
  renameContactGroup: (id: number, name: string) =>
    call<void>("rename_contact_group", { id, name }),

  /** 删分组；组内联系人回到未分组。 */
  deleteContactGroup: (id: number) => call<void>("delete_contact_group", { id }),

  /** 清空自动收集的联系人，返回删了几条。 */
  clearAutoContacts: () => call<number>("clear_auto_contacts"),

  /** 把通讯录导出到用户选定的文件。 */
  exportContacts: (path: string, kind: "csv" | "vcf", scope?: "active" | "hidden") =>
    call<ContactExport>("export_contacts", {
      path,
      kind,
      ...(scope === undefined ? {} : { scope }),
    }),

  /** 读一个导入文件并解析出预览（只读文件，不写库）。 */
  previewContactImport: (path: string, emailColumn?: number) =>
    call<ContactImportPreview>("preview_contact_import", {
      path,
      ...(emailColumn === undefined ? {} : { emailColumn }),
    }),

  /** 把预览里确认过的条目落库。 */
  applyContactImport: (entries: ContactImportEntry[], overwrite = false) =>
    call<ContactImportOutcome>("apply_contact_import", { entries, overwrite }),
  getSignature: (accountId: number) => call<Signature>("get_signature", { accountId }),

  saveSignature: (accountId: number, html: string, enabled: boolean) =>
    call<Signature>("save_signature", { accountId, html, enabled }),

  sendOutbox: () => call<SendOutcome>("send_outbox"),

  // ===== AI 与翻译（Wave 7） =====

  listAiProviders: () => call<AiProvider[]>("list_ai_providers"),

  saveAiProvider: (draft: AiProviderDraft, apiKey?: string) =>
    call<AiProvider>(
      "save_ai_provider",
      apiKey === undefined ? { draft } : { draft, apiKey },
    ),

  deleteAiProvider: (id: number) => call<void>("delete_ai_provider", { id }),

  testAiProvider: (kind: AiProviderKind, baseUrl: string, apiKey?: string, id?: number) => {
    const args: Record<string, unknown> = { kind, baseUrl };
    if (apiKey !== undefined) args.apiKey = apiKey;
    if (id !== undefined) args.id = id;
    return call<string[]>("test_ai_provider", args);
  },

  refreshAiProviderModels: (id: number) =>
    call<string[]>("refresh_ai_provider_models", { id }),

  /** 用当前选的站点和模型测一次通知识别往返耗时；内容由后端写成假邮件。 */
  checkNotificationLatency: (providerId: number, model: string, thinkingLevel?: AiThinkingLevel) =>
    call<NotificationLatency>(
      "check_notification_latency",
      thinkingLevel === undefined
        ? { providerId, model }
        : { providerId, model, thinkingLevel },
    ),

  listAiModelMaps: () => call<AiModelMap[]>("list_ai_model_maps"),

  setAiFeature: (
    fn: AiFunction,
    providerId: number,
    model: string,
    thinkingLevel?: AiThinkingLevel,
  ) =>
    call<void>(
      "set_ai_feature",
      thinkingLevel === undefined
        ? { function: fn, providerId, model }
        : { function: fn, providerId, model, thinkingLevel },
    ),

  clearAiFeature: (fn: AiFunction) => call<void>("clear_ai_feature", { function: fn }),

  aiAuthorizationPreview: (
    fn: AiFunction,
    options: { messageId?: number; targetLanguage?: string; text?: string } = {},
  ) =>
    call<AiAuthorization>("ai_authorization_preview", {
      function: fn,
      messageId: options.messageId,
      targetLanguage: options.targetLanguage ?? "",
      text: options.text,
    }),

  translateMessage: (messageId: number, targetLanguage: string, authorizationToken: string) =>
    call<AiTranslation>("translate_message", {
      messageId,
      targetLanguage,
      authorizationToken,
    }),

  summarizeMessage: (messageId: number, authorizationToken: string) =>
    call<AiTextOutcome>("summarize_message", { messageId, authorizationToken }),

  polishText: (text: string, authorizationToken: string) =>
    call<AiTextOutcome>("polish_text", { text, authorizationToken }),

  draftText: (instruction: string, authorizationToken: string) =>
    call<AiTextOutcome>("draft_text", { instruction, authorizationToken }),

  listAiAudit: (limit?: number) =>
    call<AiAudit[]>("list_ai_audit", limit === undefined ? {} : { limit }),

  disableAllAi: () => call<number>("disable_all_ai"),

  clearAiCache: () => call<number>("clear_ai_cache"),
  // ===== MCP 外部接入（Wave 8）=====

  mcpStatus: () => call<McpStatus>("mcp_status"),

  mcpSetEnabled: (enabled: boolean) => call<McpStatus>("mcp_set_enabled", { enabled }),

  mcpSetWriteTools: (enabled: boolean) => call<McpStatus>("mcp_set_write_tools", { enabled }),

  mcpTools: () => call<McpTool[]>("mcp_tools"),

  mcpAudit: (limit?: number) => call<McpAuditPage>("mcp_audit", limit === undefined ? {} : { limit }),

  // ===== GitHub 登录与设置同步（Wave S6）=====

  /** 发起一次设备码登录；`scope` 传 `sync` 时多要 Gist 权限。 */
  githubLoginStart: (scope: GitHubScope) =>
    call<GitHubDeviceLoginView>("github_login_start", { scope }),

  /** 轮询一次设备码登录结果。 */
  githubLoginPoll: (loginId: string) => call<GitHubLoginPoll>("github_login_poll", { loginId }),

  /** 退出 GitHub 登录：清令牌与资料。 */
  githubLoginSignOut: () => call<void>("github_login_sign_out"),

  /** 读本机已登录的 GitHub 资料；没登录返回 null。 */
  githubLoginProfile: () => call<GitHubLoginView | null>("github_login_profile"),

  /** 当前设置同步状态。 */
  settingsSyncStatus: () => call<SettingsSyncStatus>("settings_sync_status"),

  /** 开启设置同步。 */
  settingsSyncEnable: (password: string, confirm: string, deviceLabel: string) =>
    call<SettingsSyncStatus>("settings_sync_enable", { password, confirm, deviceLabel }),

  /** 关闭设置同步；`deleteRemote` 为真时连云端那份一起删。 */
  settingsSyncDisable: (deleteRemote: boolean) =>
    call<SettingsSyncStatus>("settings_sync_disable", { deleteRemote }),

  /** 立即同步。 */
  settingsSyncNow: () => call<SettingsSyncStatus>("settings_sync_now"),

  /** 重设同步密码。 */
  settingsSyncResetPassword: (newPassword: string, confirm: string) =>
    call<SettingsSyncStatus>("settings_sync_reset_password", { newPassword, confirm }),

  /** 新设备加入：输同步密码解密云端配置并导入。 */
  settingsSyncJoin: (password: string, deviceLabel: string, confirmOverwrite: boolean) =>
    call<SettingsSyncStatus>("settings_sync_join", { password, deviceLabel, confirmOverwrite }),

  /** 解决冲突：保留本机或按远端导入。 */
  settingsSyncResolveConflict: (choice: ConflictChoice) =>
    call<SettingsSyncStatus>("settings_sync_resolve_conflict", { choice }),
  // ===== 关于与更新 =====

  /**
   * 当前运行的版本号。
   *
   * 取的是打包时写进程序里的版本，跟安装包一致；界面上不写死版本常量。
   */
  appVersion: () => getVersion(),

  /** 查一次更新；没新版返回 null，有新版返回版本信息供界面显示。 */
  checkForUpdate: async (): Promise<UpdateInfo | null> => {
    await releasePendingUpdate();
    const update = await check();
    if (!update) return null;
    pendingUpdate = update;
    return {
      version: update.version,
      currentVersion: update.currentVersion,
      notes: update.body ?? "",
      date: update.date ?? "",
    };
  },

  /**
   * 下载并安装刚查到的更新。
   *
   * 下载完会先验签名再装：签名对不上直接失败，不会把来路不明的包装上去。
   * Windows 上安装程序起来后主程序会被结束，所以这个函数不一定还会返回。
   */
  installPendingUpdate: async (onProgress: (percent: number | null) => void): Promise<void> => {
    const update = pendingUpdate;
    if (!update) throw new Error("还没有查到可以安装的更新");
    let total = 0;
    let received = 0;
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        received = 0;
        onProgress(null);
      } else if (event.event === "Progress") {
        received += event.data.chunkLength;
        onProgress(total > 0 ? Math.min(100, Math.round((received / total) * 100)) : null);
      } else {
        onProgress(100);
      }
    });
  },

  /** 装完重启到新版本。 */
  relaunchApp: () => relaunch(),
};

// ============================ MCP 外部接入（Wave 8） ============================

/** MCP 状态与外部 Agent 配置说明；默认关闭。 */
export interface McpStatus {
  enabled: boolean;
  writeToolsEnabled: boolean;
  dataDir: string;
  binaryName: string;
  binaryPath: string;
  dataDirEnv: string;
  protocolVersions: string[];
  configExample: string;
}

/** 工具清单里的一条。 */
export interface McpTool {
  name: string;
  title: string;
  description: string;
  readOnly: boolean;
  enabled: boolean;
}

/** 一条 MCP 审计记录；只有参数哈希，没有正文。 */
export interface McpAudit {
  id: number;
  tool: string;
  accountScope: string;
  argsDigest: string;
  status: string;
  ts: string;
}

/** 审计列表。 */
export interface McpAuditPage {
  items: McpAudit[];
  total: number;
}

// ============================ GitHub 登录与设置同步（Wave S6） ============================

/** GitHub 这次登录申请到哪一档权限：只要身份，或再加 Gist。 */
export type GitHubScope = "login" | "sync";

/** 一次设备码登录给界面看的公开信息；不含能换令牌的设备码。 */
export interface GitHubDeviceLoginView {
  /** 本次登录的临时编号；轮询时带回去。 */
  loginId: string;
  /** 显示给用户输入的短码。 */
  userCode: string;
  /** 让用户打开的确认页地址。 */
  verificationUri: string;
  /** 这组码的有效秒数。 */
  expiresIn: number;
  /** GitHub 要求的轮询间隔（秒）。 */
  interval: number;
}

/** GitHub 账号资料；不含令牌。 */
export interface GitHubLoginView {
  login: string;
  name?: string | null;
  avatarUrl?: string | null;
  /**
   * 本机缓存头像的 data URL；后端登录时下载并校验过。
   * 有它就用它，省一次网络请求；没有才退回 avatarUrl。
   */
  avatarDataUrl?: string | null;
  /** 本次登录申请到哪一档权限；据此判断要不要再授权 Gist。 */
  scope: GitHubScope;
}

/** 轮询一次设备码登录的结果；`status` 就是后端那个松散的枚举。 */
export type GitHubLoginPoll =
  | { status: "pending" }
  | { status: "slowDown" }
  | { status: "expired" }
  | { status: "denied" }
  | ({ status: "authorized" } & GitHubLoginView);

/** 冲突的公开信息：哪台设备、什么时间、哪个版本。 */
export interface SettingsSyncConflict {
  remoteDeviceId: string;
  remoteDeviceLabel: string;
  remoteRevision: number;
  updatedAt: string;
}

/** 冲突处理选择。 */
export type ConflictChoice = "keepLocal" | "keepRemote";

/** 给界面看的一份同步状态（不含任何密钥）。 */
export interface SettingsSyncStatus {
  enabled: boolean;
  deviceId?: string | null;
  deviceLabel?: string | null;
  gistId?: string | null;
  gistUrl?: string | null;
  revision: number;
  remoteRevision: number;
  lastSyncAt?: string | null;
  lastError?: string | null;
  conflict?: SettingsSyncConflict | null;
}
