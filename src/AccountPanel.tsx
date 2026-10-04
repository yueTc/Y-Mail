import { useCallback, useEffect, useMemo, useState } from "react";

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

/** 常用邮箱服务商的服务器参数预设（Wave 1 先覆盖国内常用的几家）。 */
const PRESETS: Record<string, { label: string; imap: ServerConfig; smtp: ServerConfig; note: string }> = {
  "qq.com": {
    label: "QQ 邮箱",
    imap: { host: "imap.qq.com", port: 993, security: "tls" },
    smtp: { host: "smtp.qq.com", port: 465, security: "tls" },
    note: "需要在 QQ 邮箱网页版的「设置 → 账户」里开启 IMAP/SMTP 服务，并生成授权码；登录名填完整邮箱地址。",
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
    note: "需要先在 Google 账号里开启两步验证，再生成「应用专用密码」；完整的 OAuth2 登录在 Wave 6 支持。",
  },
  "outlook.com": {
    label: "Outlook / Hotmail",
    imap: { host: "outlook.office365.com", port: 993, security: "tls" },
    smtp: { host: "smtp.office365.com", port: 587, security: "starttls" },
    note: "微软已逐步停用应用密码，若登录失败请等 Wave 6 的 OAuth2 支持。",
  },
  "hotmail.com": {
    label: "Outlook / Hotmail",
    imap: { host: "outlook.office365.com", port: 993, security: "tls" },
    smtp: { host: "smtp.office365.com", port: 587, security: "starttls" },
    note: "微软已逐步停用应用密码，若登录失败请等 Wave 6 的 OAuth2 支持。",
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

const PROXY_MODE_LABELS: Record<AccountProxyMode, string> = {
  inherit: "跟随全局",
  direct: "直连",
  custom: "指定代理",
};

function presetFor(email: string): (typeof PRESETS)[string] | undefined {
  const at = email.lastIndexOf("@");
  if (at < 0) return undefined;
  return PRESETS[email.slice(at + 1).trim().toLowerCase()];
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
  if (!form.email.includes("@")) return "请填写完整的邮箱地址";
  if (form.authType === "oauth2") {
    if (form.oauthProvider === "") return "OAuth2 登录要先选服务商（Gmail 或 Outlook）";
    if (form.oauthClientId.trim() === "") {
      return "OAuth2 登录要填客户端编号：先在服务商后台注册一个桌面应用才能拿到";
    }
  }
  if (form.username.trim() === "") return "请填写登录名（多数邮箱就是完整地址）";
  if (form.imap.host.trim() === "") return "请填写收件服务器地址";
  if (form.smtp.host.trim() === "") return "请填写发件服务器地址";
  if (form.imap.port < 1 || form.imap.port > 65535) return "收件端口要在 1 到 65535 之间";
  if (form.smtp.port < 1 || form.smtp.port > 65535) return "发件端口要在 1 到 65535 之间";
  if (form.proxyMode === "custom" && form.proxyId === "") return "选了「指定代理」就要挑一个具体代理";
  if (form.authType !== "oauth2" && form.id === null && form.secret.trim() === "") {
    return "请填写授权码";
  }
  return null;
}

interface Props {
  /** 代理列表变化时由外壳递增，用来触发本面板重新拉取可选代理。 */
  proxiesVersion: number;
}

export default function AccountPanel({ proxiesVersion }: Props) {
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [proxies, setProxies] = useState<Proxy[]>([]);
  const [form, setForm] = useState<FormState | null>(null);
  const [report, setReport] = useState<ConnectionReport | null>(null);
  const [verifiedFingerprint, setVerifiedFingerprint] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pendingAuth, setPendingAuth] = useState<OAuthAuthorization | null>(null);
  const [oauthStatus, setOauthStatus] = useState<OAuthStatus | null>(null);

  const fingerprint = useMemo(() => (form ? JSON.stringify(form) : ""), [form]);
  const dirty = form !== null && verifiedFingerprint !== fingerprint;

  const reload = useCallback(async () => {
    try {
      const list = await api.listAccounts();
      setAccounts(list);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

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
    if (form === null || form.id === null || form.authType !== "oauth2") {
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
  }, [form?.id, form?.authType]);

  function openCreate() {
    setForm(emptyForm());
    setReport(null);
    setVerifiedFingerprint(null);
    setError(null);
    setNotice(null);
    setPendingAuth(null);
    setOauthStatus(null);
  }

  function openEdit(account: Account) {
    setForm(formFromAccount(account));
    setReport(null);
    setVerifiedFingerprint(null);
    setError(null);
    setNotice(null);
    setPendingAuth(null);
    setOauthStatus(null);
  }

  function closeForm() {
    // 还没收口的授权要显式取消，免得本机回调端口一直挂着。
    if (pendingAuth !== null) {
      void api.cancelOAuthAuthorize(pendingAuth.state).catch(() => undefined);
    }
    setForm(null);
    setReport(null);
    setVerifiedFingerprint(null);
    setError(null);
    setPendingAuth(null);
    setOauthStatus(null);
  }

  function patch(change: Partial<FormState>) {
    setForm((current) => (current === null ? null : { ...current, ...change }));
    setNotice(null);
  }

  function patchServer(side: "imap" | "smtp", change: Partial<ServerConfig>) {
    setForm((current) => {
      if (current === null) return null;
      const next = { ...current[side], ...change };
      return { ...current, [side]: next };
    });
    setNotice(null);
  }

  function applyPreset() {
    if (form === null) return;
    const preset = presetFor(form.email);
    if (!preset) {
      setError("没认出这个邮箱服务商，请手动填写服务器参数");
      return;
    }
    setForm({
      ...form,
      imap: { ...preset.imap },
      smtp: { ...preset.smtp },
      username: form.username.trim() === "" ? form.email.trim() : form.username,
    });
    setError(null);
    setNotice(`已按「${preset.label}」填入服务器参数：${preset.note}`);
  }

  async function handleTest() {
    if (form === null) return;
    if (form.authType === "oauth2") {
      setError(
        "OAuth2 账号不走授权码自检：点「浏览器授权」，授权成功后引擎会自动做一次连接自检。",
      );
      return;
    }
    if (form.id !== null && form.secret.trim() === "") {
      setError(
        "要自检这份改动，请先在「新的授权码」里填一次；不想重填就直接点「保存」，引擎会用已保存的授权码先自检、通过才写入。",
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
      setNotice("自检通过：收件和发件服务器都能正常登录。");
    } catch (err) {
      setReport(null);
      setVerifiedFingerprint(null);
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleSave() {
    if (form === null) return;
    const problem = validate(form);
    if (problem) {
      setError(problem);
      return;
    }
    if (form.authType === "oauth2" && form.id === null) {
      setError("OAuth2 账号请点「浏览器授权」：授权通过后账号会自动保存，不用走「保存」。");
      return;
    }
    if (form.authType !== "oauth2" && form.id === null && (dirty || report === null)) {
      setError("新建账号要先点「连接自检」，通过之后才能保存");
      return;
    }
    setBusy("save");
    setError(null);
    setNotice(null);
    try {
      const draft = toDraft(form);
      if (form.id === null) {
        await api.createAccount(draft, form.secret);
        setNotice("账号已保存；保存前已完成连接自检。");
      } else {
        const secret =
          form.authType === "oauth2" || form.secret.trim() === "" ? undefined : form.secret;
        await api.updateAccount(form.id, draft, secret);
        setNotice("账号已更新；引擎在写入前完成了一次连接自检。");
      }
      setForm(null);
      setReport(null);
      setVerifiedFingerprint(null);
      await reload();
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
    if (form === null) return;
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
      setNotice("已尝试打开系统浏览器，请在授权页点同意；本窗口正在等待回调……");
      const outcome = await api.completeOAuthAuthorize(authorization.state);
      setPendingAuth(null);
      setNotice(
        `授权成功：收件服务器可见 ${outcome.report.imapFolderCount} 个文件夹，发件认证方式 ${outcome.report.smtpMechanism}。`,
      );
      await reload();
      if (wasNew) {
        setForm(null);
        setReport(null);
        setVerifiedFingerprint(null);
        setOauthStatus(null);
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

  async function handleSavedTest(account: Account) {
    setBusy(`test-${account.id}`);
    setError(null);
    setNotice(null);
    try {
      const result = await api.testSavedAccount(account.id);
      setNotice(
        `「${account.displayName}」自检通过：收件服务器能看到 ${result.imapFolderCount} 个文件夹，发件认证方式 ${result.smtpMechanism}。`,
      );
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleDelete(account: Account) {
    if (!window.confirm(`确定删除账号「${account.displayName}」吗？系统凭据管理器里的授权码会一并删除。`)) {
      return;
    }
    setBusy(`delete-${account.id}`);
    setError(null);
    setNotice(null);
    try {
      await api.deleteAccount(account.id);
      if (form !== null && form.id === account.id) closeForm();
      setNotice(`已删除账号「${account.displayName}」。`);
      await reload();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  const preset = form ? presetFor(form.email) : undefined;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>邮箱账号</h2>
        {form === null && (
          <button type="button" onClick={openCreate}>
            新增账号
          </button>
        )}
      </div>

      {error && <p className="error">{error}</p>}
      {notice && <p className="notice">{notice}</p>}

      {accounts === null ? (
        <p className="hint">正在读取账号……</p>
      ) : accounts.length === 0 ? (
        <p className="hint">还没有账号。点「新增账号」填一个，保存前会先做连接自检。</p>
      ) : (
        <ul className="card-list">
          {accounts.map((account) => (
            <li key={account.id} className="card">
              <span className="dot" style={{ background: account.color || "#3b82f6" }} />
              <div className="card-main">
                <div className="card-title">
                  {account.displayName}
                  {!account.enabled && <span className="tag">已停用</span>}
                  <span className="tag">{PROXY_MODE_LABELS[account.proxy.mode]}</span>
                  {!account.hasCredential && <span className="tag warn">缺授权码</span>}
                </div>
                <div className="card-sub">
                  {account.email}
                  {account.username !== account.email ? `（登录名 ${account.username}）` : ""}
                </div>
                <div className="card-sub path">
                  收 {account.imap.host}:{account.imap.port} · 发 {account.smtp.host}:{account.smtp.port}
                </div>
              </div>
              <div className="card-actions">
                <button type="button" onClick={() => openEdit(account)} disabled={busy !== null}>
                  编辑
                </button>
                <button
                  type="button"
                  onClick={() => void handleSavedTest(account)}
                  disabled={busy !== null}
                >
                  {busy === `test-${account.id}` ? "自检中……" : "自检"}
                </button>
                <button
                  type="button"
                  className="danger"
                  onClick={() => void handleDelete(account)}
                  disabled={busy !== null}
                >
                  {busy === `delete-${account.id}` ? "删除中……" : "删除"}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {form !== null && (
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSave();
          }}
        >
          <h3>{form.id === null ? "新增账号" : `编辑账号 #${form.id}`}</h3>

          <div className="field-row">
            <label>
              显示名
              <input
                value={form.displayName}
                onChange={(event) => patch({ displayName: event.target.value })}
                placeholder="可留空，默认用邮箱地址"
              />
            </label>
            <label>
              邮箱地址
              <input
                value={form.email}
                onChange={(event) => patch({ email: event.target.value })}
                placeholder="name@example.com"
              />
            </label>
            <label>
              &nbsp;
              <button type="button" onClick={applyPreset} disabled={!preset}>
                {preset ? `按「${preset.label}」填服务器` : "常用邮箱自动填"}
              </button>
            </label>
          </div>

          <div className="field-row">
            <label>
              登录名
              <input
                value={form.username}
                onChange={(event) => patch({ username: event.target.value })}
                placeholder="多数邮箱就是完整地址"
              />
            </label>
            <label>
              认证方式
              <select
                value={form.authType}
                onChange={(event) => patch({ authType: event.target.value as AuthType })}
              >
                {(Object.keys(AUTH_LABELS) as AuthType[]).map((value) => (
                  <option key={value} value={value}>
                    {AUTH_LABELS[value]}
                  </option>
                ))}
              </select>
            </label>
            <label>
              色标
              <input
                type="color"
                value={form.color || "#3b82f6"}
                onChange={(event) => patch({ color: event.target.value })}
              />
            </label>
          </div>

          <div className="field-row">
            <label>
              收件服务器
              <input
                value={form.imap.host}
                onChange={(event) => patchServer("imap", { host: event.target.value })}
                placeholder="imap.example.com"
              />
            </label>
            <label>
              收件端口
              <input
                type="number"
                min={1}
                max={65535}
                value={form.imap.port}
                onChange={(event) => patchServer("imap", { port: Number(event.target.value) })}
              />
            </label>
            <label>
              收件加密
              <select
                value={form.imap.security}
                onChange={(event) => patchServer("imap", { security: event.target.value as Security })}
              >
                {(Object.keys(SECURITY_LABELS) as Security[]).map((value) => (
                  <option key={value} value={value}>
                    {SECURITY_LABELS[value]}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <div className="field-row">
            <label>
              发件服务器
              <input
                value={form.smtp.host}
                onChange={(event) => patchServer("smtp", { host: event.target.value })}
                placeholder="smtp.example.com"
              />
            </label>
            <label>
              发件端口
              <input
                type="number"
                min={1}
                max={65535}
                value={form.smtp.port}
                onChange={(event) => patchServer("smtp", { port: Number(event.target.value) })}
              />
            </label>
            <label>
              发件加密
              <select
                value={form.smtp.security}
                onChange={(event) => patchServer("smtp", { security: event.target.value as Security })}
              >
                {(Object.keys(SECURITY_LABELS) as Security[]).map((value) => (
                  <option key={value} value={value}>
                    {SECURITY_LABELS[value]}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <div className="field-row">
            <label>
              代理策略
              <select
                value={form.proxyMode}
                onChange={(event) => patch({ proxyMode: event.target.value as AccountProxyMode })}
              >
                {(Object.keys(PROXY_MODE_LABELS) as AccountProxyMode[]).map((value) => (
                  <option key={value} value={value}>
                    {PROXY_MODE_LABELS[value]}
                  </option>
                ))}
              </select>
            </label>
            {form.proxyMode === "custom" && (
              <label>
                指定代理
                <select
                  value={form.proxyId}
                  onChange={(event) => patch({ proxyId: event.target.value })}
                >
                  <option value="">请选择</option>
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
              启用这个账号
            </label>
          </div>

          {form.authType === "oauth2" ? (
            <>
              <div className="field-row">
                <label>
                  OAuth2 服务商
                  <select
                    value={form.oauthProvider}
                    onChange={(event) =>
                      patch({ oauthProvider: event.target.value as OAuthProvider | "" })
                    }
                  >
                    <option value="">请选择</option>
                    <option value="gmail">谷歌 Gmail</option>
                    <option value="microsoft">微软 Outlook</option>
                  </select>
                </label>
                <label>
                  客户端编号（client_id）
                  <input
                    value={form.oauthClientId}
                    onChange={(event) => patch({ oauthClientId: event.target.value })}
                    placeholder="在服务商后台注册桌面应用后拿到"
                  />
                </label>
              </div>
              <p className="hint">
                OAuth2 不走授权码：点「浏览器授权」会用系统浏览器打开服务商的授权页，同意后本窗口自动收口；
                访问令牌与刷新令牌只存进 Windows 凭据管理器，数据库里只留一个引用键。
              </p>
              {oauthStatus && (
                <p className="notice">
                  授权状态：{oauthStatus.authorized ? "已授权" : "尚未授权"}
                  {oauthStatus.hasRefreshToken ? "，令牌到期会自动刷新" : "，没有刷新令牌，过期后要重新授权"}
                  。
                </p>
              )}
              {pendingAuth && (
                <>
                  <p className="hint">
                    正在等待浏览器回调。如果浏览器没有自动打开，请手工复制下面这行地址到浏览器打开：
                  </p>
                  <textarea className="authorize-url" readOnly rows={3} value={pendingAuth.authorizeUrl} />
                </>
              )}
            </>
          ) : (
            <>
              <label className="secret-field">
                {form.id === null ? "授权码 / 密码" : "新的授权码（留空表示沿用已保存的）"}
                <input
                  type="password"
                  autoComplete="off"
                  value={form.secret}
                  onChange={(event) => patch({ secret: event.target.value })}
                  placeholder={form.id === null ? "只保存在 Windows 凭据管理器" : "不填就不改"}
                />
              </label>
              <p className="hint">
                授权码只在这个输入框和本次请求里存在，不写数据库、不进日志；数据库里只留一个引用键。
              </p>
            </>
          )}

          {report && (
            <p className="notice">
              上次自检结果：收件服务器可见 {report.imapFolderCount} 个文件夹，发件认证方式{" "}
              {report.smtpMechanism}。
              {dirty ? "（表单已改动，请重新自检）" : ""}
            </p>
          )}
          {form.id !== null && (
            <p className="hint">保存时会先做一次连接自检；不通过就不会写入，原来的账号保持不变。</p>
          )}

          <div className="form-actions">
            {form.authType === "oauth2" ? (
              <button
                type="button"
                className="primary"
                onClick={() => void handleAuthorize()}
                disabled={busy !== null}
              >
                {busy === "oauth" ? "等待浏览器授权……" : form.id === null ? "浏览器授权并保存" : "重新授权"}
              </button>
            ) : (
              <button type="button" onClick={() => void handleTest()} disabled={busy !== null}>
                {busy === "test" ? "自检中……" : form.id === null ? "连接自检" : "用上面的授权码自检"}
              </button>
            )}
            <button
              type="submit"
              className="primary"
              disabled={busy !== null || (form.authType === "oauth2" && form.id === null)}
            >
              {busy === "save" ? "保存中……" : "保存"}
            </button>
            <button type="button" onClick={closeForm} disabled={busy !== null}>
              取消
            </button>
          </div>
        </form>
      )}
    </section>
  );
}