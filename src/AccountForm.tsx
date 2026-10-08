import { useEffect, useMemo, useState } from "react";

import {
  api,
  describeError,
  type Account,
  type AccountDraft,
  type AccountProxyMode,
  type AuthType,
  type ConnectionReport,
  type OAuthAuthorization,
  type OAuthProvider,
  type OAuthStatus,
  type Proxy,
  type Security,
  type ServerConfig,
} from "./api";
import type { AddAccountSource } from "./AddAccountDialog";
import { t } from "./i18n";

/** 表单状态；授权码只存在这个内存对象里，不写 localStorage、不回显已保存的值。 */
interface FormState {
  id: number | null;
  displayName: string;
  email: string;
  authType: AuthType;
  username: string;
  imap: ServerConfig;
  smtp: ServerConfig;
  proxyMode: AccountProxyMode;
  proxyId: string;
  color: string;
  enabled: boolean;
  secret: string;
  oauthProvider: OAuthProvider | "";
  oauthClientId: string;
}

const DEFAULT_SERVERS: { imap: ServerConfig; smtp: ServerConfig } = {
  imap: { host: "imap.example.com", port: 993, security: "tls" },
  smtp: { host: "smtp.example.com", port: 465, security: "tls" },
};

/** QQ 系邮箱共用同一组服务器参数。 */
const QQ_SERVERS: { imap: ServerConfig; smtp: ServerConfig } = {
  imap: { host: "imap.qq.com", port: 993, security: "tls" },
  smtp: { host: "smtp.qq.com", port: 465, security: "tls" },
};

const QQ_NOTE =
  "需要在 QQ 邮箱网页版的「设置 → 账户」里开启 IMAP/SMTP 服务，并生成授权码；登录名填完整邮箱地址。";

/** 常用邮箱服务商的服务器参数预设（Wave 1 先覆盖国内常用的几家）。 */
const PRESETS: Record<
  string,
  { label: string; imap: ServerConfig; smtp: ServerConfig; note: string; oauthProvider?: OAuthProvider }
> = {
  "qq.com": {
    label: "QQ 邮箱",
    ...QQ_SERVERS,
    note: QQ_NOTE,
  },
  "foxmail.com": {
    label: "Foxmail 邮箱",
    ...QQ_SERVERS,
    note: QQ_NOTE,
  },
  "vip.qq.com": {
    label: "QQ 邮箱（VIP）",
    ...QQ_SERVERS,
    note: QQ_NOTE,
  },
  "163.com": {
    label: "网易 163 邮箱",
    imap: { host: "imap.163.com", port: 993, security: "tls" },
    smtp: { host: "smtp.163.com", port: 465, security: "tls" },
    note: "需要在网页版邮箱「设置 → POP3/SMTP/IMAP」里开启服务并获取授权码。",
  },
  "126.com": {
    label: "网易 126 邮箱",
    imap: { host: "imap.126.com", port: 993, security: "tls" },
    smtp: { host: "smtp.126.com", port: 465, security: "tls" },
    note: "需要在网页版邮箱「设置 → POP3/SMTP/IMAP」里开启服务并获取授权码。",
  },
  "gmail.com": {
    label: "Gmail",
    imap: { host: "imap.gmail.com", port: 993, security: "tls" },
    smtp: { host: "smtp.gmail.com", port: 465, security: "tls" },
    note: "谷歌邮箱会自动改用 OAuth2；点「浏览器授权」用系统默认浏览器登录即可。",
    oauthProvider: "gmail",
  },
  "outlook.com": {
    label: "Outlook / Hotmail",
    imap: { host: "outlook.office365.com", port: 993, security: "tls" },
    smtp: { host: "smtp.office365.com", port: 587, security: "starttls" },
    note: "微软已停用应用密码，这里会自动改用 OAuth2；点「浏览器授权」用系统默认浏览器登录即可。",
    oauthProvider: "microsoft",
  },
  "hotmail.com": {
    label: "Outlook / Hotmail",
    imap: { host: "outlook.office365.com", port: 993, security: "tls" },
    smtp: { host: "smtp.office365.com", port: 587, security: "starttls" },
    note: "微软已停用应用密码，这里会自动改用 OAuth2；点「浏览器授权」用系统默认浏览器登录即可。",
    oauthProvider: "microsoft",
  },
  "live.com": {
    label: "Outlook / Hotmail",
    imap: { host: "outlook.office365.com", port: 993, security: "tls" },
    smtp: { host: "smtp.office365.com", port: 587, security: "starttls" },
    note: "微软已停用应用密码，这里会自动改用 OAuth2；点「浏览器授权」用系统默认浏览器登录即可。",
    oauthProvider: "microsoft",
  },
  "msn.com": {
    label: "Outlook / Hotmail",
    imap: { host: "outlook.office365.com", port: 993, security: "tls" },
    smtp: { host: "smtp.office365.com", port: 587, security: "starttls" },
    note: "微软已停用应用密码，这里会自动改用 OAuth2；点「浏览器授权」用系统默认浏览器登录即可。",
    oauthProvider: "microsoft",
  },
  "icloud.com": {
    label: "iCloud 邮箱",
    imap: { host: "imap.mail.me.com", port: 993, security: "tls" },
    smtp: { host: "smtp.mail.me.com", port: 587, security: "starttls" },
    note: "需要先在 Apple ID 里生成「App 专用密码」。",
  },
};
const SECURITY_LABELS: Record<Security, string> = {
  tls: "SSL/TLS（隐式加密）",
  starttls: "STARTTLS（先明文后升级）",
  plain: "不加密",
};

const AUTH_LABELS: Record<AuthType, string> = {
  password: "授权码 / 密码",
  oauth2: "OAuth2（浏览器授权）",
};

/** 代理策略的中文标签；账号面板列账号时也用同一份。 */
export const PROXY_MODE_LABELS: Record<AccountProxyMode, string> = {
  inherit: "跟随全局",
  direct: "直连",
  custom: "指定代理",
};

function presetFor(email: string): (typeof PRESETS)[string] | undefined {
  const at = email.lastIndexOf("@");
  if (at < 0) return undefined;
  return PRESETS[email.slice(at + 1).trim().toLowerCase()];
}

function sameServerConfig(a: ServerConfig, b: ServerConfig): boolean {
  return a.host === b.host && a.port === b.port && a.security === b.security;
}

function emptyForm(): FormState {
  return {
    id: null,
    displayName: "",
    email: "",
    authType: "password",
    username: "",
    imap: { ...DEFAULT_SERVERS.imap },
    smtp: { ...DEFAULT_SERVERS.smtp },
    proxyMode: "inherit",
    proxyId: "",
    color: "#3b82f6",
    enabled: true,
    secret: "",
    oauthProvider: "",
    oauthClientId: "",
  };
}

function formFromAccount(account: Account): FormState {
  return {
    id: account.id,
    displayName: account.displayName,
    email: account.email,
    authType: account.authType,
    username: account.username,
    imap: { ...account.imap },
    smtp: { ...account.smtp },
    proxyMode: account.proxy.mode,
    proxyId: account.proxy.proxyId === undefined ? "" : String(account.proxy.proxyId),
    color: account.color || "#3b82f6",
    enabled: account.enabled,
    secret: "",
    oauthProvider: account.oauthProvider ?? "",
    oauthClientId: account.oauthClientId ?? "",
  };
}

function toDraft(form: FormState): AccountDraft {
  return {
    displayName: form.displayName.trim() || form.email.trim(),
    email: form.email.trim(),
    authType: form.authType,
    username: form.username.trim(),
    imap: { ...form.imap, host: form.imap.host.trim() },
    smtp: { ...form.smtp, host: form.smtp.host.trim() },
    proxy:
      form.proxyMode === "custom"
        ? { mode: "custom", proxyId: Number(form.proxyId) }
        : { mode: form.proxyMode },
    color: form.color,
    enabled: form.enabled,
    oauthProvider: form.authType === "oauth2" && form.oauthProvider !== "" ? form.oauthProvider : undefined,
    oauthClientId: form.authType === "oauth2" ? form.oauthClientId.trim() : "",
  };
}

/** 提交前的本地检查；只拦明显问题，真正的校验与自检在引擎里做。 */
function validate(form: FormState): string | null {
  if (!form.email.includes("@")) return t("请填写完整的邮箱地址");
  if (form.authType === "oauth2" && form.oauthProvider === "") {
    return t("OAuth2 登录要先选服务商（Gmail 或 Outlook）");
  }
  if (form.username.trim() === "") return t("请填写登录名（多数邮箱就是完整地址）");
  if (form.imap.host.trim() === "") return t("请填写收件服务器地址");
  if (form.smtp.host.trim() === "") return t("请填写发件服务器地址");
  if (form.imap.port < 1 || form.imap.port > 65535) return t("收件端口要在 1 到 65535 之间");
  if (form.smtp.port < 1 || form.smtp.port > 65535) return t("发件端口要在 1 到 65535 之间");
  if (form.proxyMode === "custom" && form.proxyId === "") return t("选了「指定代理」就要挑一个具体代理");
  if (form.authType !== "oauth2" && form.id === null && form.secret.trim() === "") {
    return t("请填写授权码");
  }
  return null;
}

export interface AccountFormProps {
  /** 要编辑的账号；传 null 表示新建。 */
  account: Account | null;
  /** 新建来源；编辑模式忽略，只影响保存后的提示文案。 */
  source?: AddAccountSource;
  /** 代理列表变化版本号，变化时重新拉取可选代理。 */
  proxiesVersion?: number;
  /** 保存成功（新建或编辑）后回调账号编号。 */
  onSaved: (accountId: string) => void;
  /** 点「取消」或请求收起表单时调用。 */
  onCancel: () => void;
  /** 表单内容有没有被改动过，供外层决定关闭前要不要确认。 */
  onDirtyChange?: (dirty: boolean) => void;
  /** 正在测试 / 保存 / 授权时通知外层，供外层决定能不能关。 */
  onBusyChange?: (busy: boolean) => void;
}

export default function AccountForm({
  account,
  source = "settings",
  proxiesVersion,
  onSaved,
  onCancel,
  onDirtyChange,
  onBusyChange,
}: AccountFormProps) {
  const [form, setForm] = useState<FormState>(() =>
    account === null ? emptyForm() : formFromAccount(account),
  );
  const [proxies, setProxies] = useState<Proxy[]>([]);
  const [report, setReport] = useState<ConnectionReport | null>(null);
  const [verifiedFingerprint, setVerifiedFingerprint] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pendingAuth, setPendingAuth] = useState<OAuthAuthorization | null>(null);
  const [oauthStatus, setOauthStatus] = useState<OAuthStatus | null>(null);
  const [touched, setTouched] = useState(false);
  /** 用户有没有自己改过服务器参数；自己改过就不再自动覆盖。 */
  const [serversEdited, setServersEdited] = useState(account !== null);
  /** 用户有没有自己改过认证方式或 OAuth2 服务商；改过就不再自动覆盖。 */
  const [authEdited, setAuthEdited] = useState(false);

  const editing = account !== null;
  const fingerprint = useMemo(() => JSON.stringify(form), [form]);
  // 表单相对「上次自检通过时的样子」有没有变，用来卡住新建账号的保存。
  const needsRetest = verifiedFingerprint !== fingerprint;

  useEffect(() => {
    onDirtyChange?.(touched);
  }, [touched, onDirtyChange]);

  useEffect(() => {
    onBusyChange?.(busy !== null);
  }, [busy, onBusyChange]);

  useEffect(() => {
    let cancelled = false;
    api
      .listProxies()
      .then((list) => {
        if (!cancelled) setProxies(list);
      })
      .catch(() => {
        if (!cancelled) setProxies([]);
      });
    return () => {
      cancelled = true;
    };
  }, [proxiesVersion]);

  // 编辑 OAuth2 账号时顺手查一次授权状态，界面上直接显示。
  useEffect(() => {
    if (form.id === null || form.authType !== "oauth2") {
      setOauthStatus(null);
      return;
    }
    let cancelled = false;
    api
      .oauthStatus(form.id)
      .then((status) => {
        if (!cancelled) setOauthStatus(status);
      })
      .catch(() => {
        if (!cancelled) setOauthStatus(null);
      });
    return () => {
      cancelled = true;
    };
  }, [form.id, form.authType]);

  function cancel() {
    // 还没收口的授权要显式取消，免得本机回调端口一直挂着。
    if (pendingAuth !== null) {
      void api.cancelOAuthAuthorize(pendingAuth.state).catch(() => undefined);
    }
    onCancel();
  }

  function patch(change: Partial<FormState>) {
    setForm((current) => ({ ...current, ...change }));
    setTouched(true);
    setNotice(null);
  }

  function patchServer(side: "imap" | "smtp", change: Partial<ServerConfig>) {
    setForm((current) => {
      const next = { ...current[side], ...change };
      return { ...current, [side]: next };
    });
    setTouched(true);
    setServersEdited(true);
    setNotice(null);
  }

  function applyPreset(options: { force?: boolean; quiet?: boolean } = {}) {
    const { force = false, quiet = false } = options;
    const preset = presetFor(form.email);
    if (!preset) {
      if (!quiet) setError(t("没认出这个邮箱服务商，请手动填写服务器参数"));
      return;
    }
    // 自动填写只在新建账号、且用户没有碰过服务器参数时生效；点按钮则是明确要求覆盖。
    if (!force && (editing || serversEdited)) return;

    const nextUsername = form.username.trim() === "" ? form.email.trim() : form.username;
    // 微软与谷歌邮箱自动改用 OAuth2；只在新建账号、且用户没手动改过认证方式时生效。
    const switchAuth =
      form.id === null && preset.oauthProvider !== undefined && (force || !authEdited);
    const nextAuthType: AuthType = switchAuth ? "oauth2" : form.authType;
    const nextOauthProvider: OAuthProvider | "" =
      switchAuth && preset.oauthProvider !== undefined ? preset.oauthProvider : form.oauthProvider;
    const unchanged =
      form.username === nextUsername &&
      sameServerConfig(form.imap, preset.imap) &&
      sameServerConfig(form.smtp, preset.smtp) &&
      form.authType === nextAuthType &&
      form.oauthProvider === nextOauthProvider;
    if (unchanged) return;

    setForm({
      ...form,
      imap: { ...preset.imap },
      smtp: { ...preset.smtp },
      username: nextUsername,
      authType: nextAuthType,
      oauthProvider: nextOauthProvider,
    });
    setTouched(true);
    setServersEdited(false);
    setError(null);
    const switched = switchAuth && form.authType !== "oauth2";
    setNotice(
      switched
        ? t("已按「{0}」填入服务器参数，并自动改用 OAuth2 浏览器授权；点「浏览器授权」登录即可。", [t(preset.label)])
        : t("已按「{0}」填入服务器参数：{1}", [t(preset.label), t(preset.note)]),
    );
  }

  async function handleTest() {
    if (form.authType === "oauth2") {
      setError(
        t("OAuth2 账号不走授权码自检：点「浏览器授权」，授权成功后引擎会自动做一次连接自检。"),
      );
      return;
    }
    if (form.id !== null && form.secret.trim() === "") {
      setError(
        t("要自检这份改动，请先在「新的授权码」里填一次；不想重填就直接点「保存」，引擎会用已保存的授权码先自检、通过才写入。"),
      );
      return;
    }
    const problem = validate(form);
    if (problem) {
      setError(problem);
      return;
    }
    setBusy("test");
    setError(null);
    setNotice(null);
    try {
      const result = await api.testAccountConnection(toDraft(form), form.secret);
      setReport(result);
      setVerifiedFingerprint(fingerprint);
      setNotice(t("自检通过：收件和发件服务器都能正常登录。"));
    } catch (err) {
      setReport(null);
      setVerifiedFingerprint(null);
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleSave() {
    const problem = validate(form);
    if (problem) {
      setError(problem);
      return;
    }
    if (form.authType === "oauth2" && form.id === null) {
      setError(t("OAuth2 账号请点「浏览器授权」：授权通过后账号会自动保存，不用走「保存」。"));
      return;
    }
    if (form.authType !== "oauth2" && form.id === null && (needsRetest || report === null)) {
      setError(t("新建账号要先点「连接自检」，通过之后才能保存"));
      return;
    }
    setBusy("save");
    setError(null);
    setNotice(null);
    try {
      const draft = toDraft(form);
      if (form.id === null) {
        const saved = await api.createAccount(draft, form.secret);
        setNotice(t("账号已保存；保存前已完成连接自检。"));
        onSaved(String(saved.id));
      } else {
        const secret =
          form.authType === "oauth2" || form.secret.trim() === "" ? undefined : form.secret;
        const saved = await api.updateAccount(form.id, draft, secret);
        setNotice(t("账号已更新；引擎在写入前完成了一次连接自检。"));
        onSaved(String(saved.id));
      }
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  /**
   * OAuth2 浏览器授权：先让引擎开好本机回调端口并拿到授权地址，再用系统浏览器打开；
   * 浏览器里点完同意，本窗口这边收口换令牌。新建账号授权通过后会自动落库。
   */
  async function handleAuthorize() {
    const problem = validate(form);
    if (problem) {
      setError(problem);
      return;
    }
    const wasNew = form.id === null;
    let pendingState: string | null = null;
    setBusy("oauth");
    setError(null);
    setNotice(null);
    setPendingAuth(null);
    try {
      const authorization = await api.beginOAuthAuthorize(toDraft(form), form.id ?? undefined);
      pendingState = authorization.state;
      setPendingAuth(authorization);
      setNotice(t("已用系统默认浏览器打开授权页；登录并同意后，本窗口会自动收口……"));
      const outcome = await api.completeOAuthAuthorize(authorization.state);
      setPendingAuth(null);
      setNotice(
        t("授权成功：收件服务器可见 {0} 个文件夹，发件认证方式 {1}。", [outcome.report.imapFolderCount, outcome.report.smtpMechanism]),
      );
      if (wasNew) {
        onSaved(String(outcome.account.id));
      } else {
        setOauthStatus(await api.oauthStatus(outcome.account.id));
      }
    } catch (err) {
      // 收口失败时后端已经清掉了半成品；这里只把还没收口的授权取消掉。
      if (pendingState !== null) {
        void api.cancelOAuthAuthorize(pendingState).catch(() => undefined);
      }
      setPendingAuth(null);
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  const preset = presetFor(form.email);

  return (
    <form
      aria-busy={busy !== null}
      className="form"
      onSubmit={(event) => {
        event.preventDefault();
        void handleSave();
      }}
    >
      {!editing && (
        <p className="hint">
          {source === "mailbox"
            ? t("保存后会回到收件箱，并选中这个新账号。")
            : t("保存后会刷新设置页的账号列表。")}
        </p>
      )}

      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}

      <div className="field-row">
        <label>
          {t("显示名")}<input
            value={form.displayName}
            onChange={(event) => patch({ displayName: event.target.value })}
            placeholder={t("可留空，默认用邮箱地址")}
          />
        </label>
        <label>
          {t("邮箱地址")}<input
            value={form.email}
            onChange={(event) => patch({ email: event.target.value })}
            onBlur={() => applyPreset({ quiet: true })}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              applyPreset({ quiet: true });
            }}
            placeholder="name@example.com"
          />
        </label>
        <label>
          &nbsp;
          <button type="button" onClick={() => applyPreset({ force: true })} disabled={!preset}>
            {preset ? t("按「{0}」填服务器", [t(preset.label)]) : t("常用邮箱自动填")}
          </button>
        </label>
      </div>

      <div className="field-row">
        <label>
          {t("登录名")}<input
            value={form.username}
            onChange={(event) => patch({ username: event.target.value })}
            placeholder={t("多数邮箱就是完整地址")}
          />
        </label>
        <label>
          {t("认证方式")}<select
            value={form.authType}
            onChange={(event) => {
              setAuthEdited(true);
              patch({ authType: event.target.value as AuthType });
            }}
          >
            {(Object.keys(AUTH_LABELS) as AuthType[]).map((value) => (
              <option key={value} value={value}>
                {t(AUTH_LABELS[value])}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("色标")}<input
            type="color"
            value={form.color || "#3b82f6"}
            onChange={(event) => patch({ color: event.target.value })}
          />
        </label>
      </div>

      <div className="field-row">
        <label>
          {t("收件服务器")}<input
            value={form.imap.host}
            onChange={(event) => patchServer("imap", { host: event.target.value })}
            placeholder="imap.example.com"
          />
        </label>
        <label>
          {t("收件端口")}<input
            type="number"
            min={1}
            max={65535}
            value={form.imap.port}
            onChange={(event) => patchServer("imap", { port: Number(event.target.value) })}
          />
        </label>
        <label>
          {t("收件加密")}<select
            value={form.imap.security}
            onChange={(event) => patchServer("imap", { security: event.target.value as Security })}
          >
            {(Object.keys(SECURITY_LABELS) as Security[]).map((value) => (
              <option key={value} value={value}>
                {t(SECURITY_LABELS[value])}
              </option>
            ))}
          </select>
        </label>
      </div>

      <div className="field-row">
        <label>
          {t("发件服务器")}<input
            value={form.smtp.host}
            onChange={(event) => patchServer("smtp", { host: event.target.value })}
            placeholder="smtp.example.com"
          />
        </label>
        <label>
          {t("发件端口")}<input
            type="number"
            min={1}
            max={65535}
            value={form.smtp.port}
            onChange={(event) => patchServer("smtp", { port: Number(event.target.value) })}
          />
        </label>
        <label>
          {t("发件加密")}<select
            value={form.smtp.security}
            onChange={(event) => patchServer("smtp", { security: event.target.value as Security })}
          >
            {(Object.keys(SECURITY_LABELS) as Security[]).map((value) => (
              <option key={value} value={value}>
                {t(SECURITY_LABELS[value])}
              </option>
            ))}
          </select>
        </label>
      </div>

      <div className="field-row">
        <label>
          {t("代理策略")}<select
            value={form.proxyMode}
            onChange={(event) => patch({ proxyMode: event.target.value as AccountProxyMode })}
          >
            {(Object.keys(PROXY_MODE_LABELS) as AccountProxyMode[]).map((value) => (
              <option key={value} value={value}>
                {t(PROXY_MODE_LABELS[value])}
              </option>
            ))}
          </select>
        </label>
        {form.proxyMode === "custom" && (
          <label>
            {t("指定代理")}<select
              value={form.proxyId}
              onChange={(event) => patch({ proxyId: event.target.value })}
            >
              <option value="">{t("请选择")}</option>
              {proxies.map((proxy) => (
                <option key={proxy.id} value={String(proxy.id)}>
                  {proxy.label || `${proxy.host}:${proxy.port}`}
                </option>
              ))}
            </select>
          </label>
        )}
        <label className="checkbox">
          <input
            type="checkbox"
            checked={form.enabled}
            onChange={(event) => patch({ enabled: event.target.checked })}
          />
          {t("启用这个账号")}</label>
      </div>

      {form.authType === "oauth2" ? (
        <>
          <div className="field-row">
            <label>
              {t("OAuth2 服务商")}<select
                value={form.oauthProvider}
                onChange={(event) => {
                  setAuthEdited(true);
                  patch({ oauthProvider: event.target.value as OAuthProvider | "" });
                }}
              >
                <option value="">{t("请选择")}</option>
                <option value="gmail">{t("谷歌 Gmail")}</option>
                <option value="microsoft">{t("微软 Outlook")}</option>
              </select>
            </label>
          </div>
          <details className="advanced-settings">
            <summary>{t("高级设置：自备客户端编号（普通用户不用管）")}</summary>
            <label>
              {t("客户端编号（client_id）")}<input
                value={form.oauthClientId}
                onChange={(event) => patch({ oauthClientId: event.target.value })}
                placeholder={t("留空就用软件内置的编号")}
              />
            </label>
            <p className="hint">
              {t("只有你自己注册了应用、想用自己的编号时才填。留空时软件会用它内置的编号。")}</p>
          </details>
          <p className="hint">
            {t("点「浏览器授权」会用系统默认浏览器打开服务商的登录页，登录并同意后自动回到本窗口； 访问令牌与刷新令牌只存进 Windows 凭据管理器，数据库里只留一个引用键。")}</p>
          {oauthStatus && (
            <p className="notice" role="status">
              {t("授权状态：")}{oauthStatus.authorized ? t("已授权") : t("尚未授权")}
              {oauthStatus.hasRefreshToken ? t("，令牌到期会自动刷新") : t("，没有刷新令牌，过期后要重新授权")}
              {t("。")}</p>
          )}
          {pendingAuth && (
            <>
              <p className="hint">
                {t("正在等待浏览器回调。如果浏览器没有自动打开，请手工复制下面这行地址到浏览器打开：")}</p>
              <textarea className="authorize-url" readOnly rows={3} value={pendingAuth.authorizeUrl} />
            </>
          )}
        </>
      ) : (
        <>
          <label className="secret-field">
            {form.id === null ? t("授权码 / 密码") : t("新的授权码（留空表示沿用已保存的）")}
            <input
              type="password"
              autoComplete="off"
              value={form.secret}
              onChange={(event) => patch({ secret: event.target.value })}
              placeholder={form.id === null ? t("只保存在 Windows 凭据管理器") : t("不填就不改")}
            />
          </label>
          <p className="hint">
            {t("授权码只在这个输入框和本次请求里存在，不写数据库、不进日志；数据库里只留一个引用键。")}</p>
        </>
      )}

      {report && (
        <p className="notice" role="status">
          {t("上次自检结果：收件服务器可见 ")}
          {report.imapFolderCount}
          {t(" 个文件夹，发件认证方式")}{" "}
          {report.smtpMechanism}{t("。")}{needsRetest ? t("（表单已改动，请重新自检）") : ""}
        </p>
      )}
      {form.id !== null && (
        <p className="hint">{t("保存时会先做一次连接自检；不通过就不会写入，原来的账号保持不变。")}</p>
      )}

      <div className="form-actions">
        {form.authType === "oauth2" ? (
          <button
            type="button"
            className="primary"
            onClick={() => void handleAuthorize()}
            aria-busy={busy === "oauth"}
            disabled={busy !== null}
          >
            {busy === "oauth" ? t("等待浏览器授权……") : form.id === null ? t("浏览器授权并保存") : t("重新授权")}
          </button>
        ) : (
          <button type="button" aria-busy={busy === "test"} onClick={() => void handleTest()} disabled={busy !== null}>
            {busy === "test" ? t("自检中……") : form.id === null ? t("连接自检") : t("用上面的授权码自检")}
          </button>
        )}
        <button
          type="submit"
          className="primary"
          aria-busy={busy === "save"}
          disabled={busy !== null || (form.authType === "oauth2" && form.id === null)}
        >
          {busy === "save" ? t("保存中……") : t("保存")}
        </button>
        <button type="button" onClick={cancel} disabled={busy !== null}>
          {t("取消")}</button>
      </div>
    </form>
  );
}
