//! 前端调用 Tauri 命令的类型化封装。
//!
//! 约定：授权码与代理密码只在这个文件里作为函数参数传递，不落任何本地存储，
//! 界面出参里也永远拿不回已保存的密码本体（只有 hasCredential / hasPassword 布尔值）。

import { invoke } from "@tauri-apps/api/core";

// ============================ 类型 ============================

/** 认证方式；Wave 1 只用 password（授权码），oauth2 留给 Wave 6。 */
export type AuthType = "password" | "oauth2";

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
  hasCredential: boolean;
  createdAt: string;
  updatedAt: string;
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

  listProxies: () => call<Proxy[]>("list_proxies"),

  saveProxy: (config: ProxyConfig, password?: string) =>
    call<Proxy>("save_proxy", password === undefined ? { config } : { config, password }),

  deleteProxy: (id: number) => call<void>("delete_proxy", { id }),

  getProxySettings: () => call<GlobalProxy>("get_proxy_settings"),

  setProxySettings: (mode: GlobalProxy) => call<GlobalProxy>("set_proxy_settings", { mode }),

  testProxy: (id: number, target?: string) =>
    call<void>("test_proxy", target === undefined ? { id } : { id, target }),
};