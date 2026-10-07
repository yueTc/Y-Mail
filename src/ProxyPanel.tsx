import { useCallback, useEffect, useState } from "react";

import {
  api,
  describeError,
  type GlobalProxy,
  type GlobalProxyMode,
  type Proxy,
  type ProxyConfig,
  type ProxyKind,
} from "./api";

/** 表单状态；密码只存在内存里，编辑时留空表示不修改。 */
interface FormState {
  id: number | null;
  label: string;
  kind: ProxyKind;
  host: string;
  port: number;
  username: string;
  password: string;
  clearPassword: boolean;
}

const PROXY_KIND_LABELS: Record<ProxyKind, string> = {
  socks5: "SOCKS5（袜子五）",
  http: "HTTP 隧道（CONNECT）",
};

const GLOBAL_MODE_LABELS: Record<GlobalProxyMode, string> = {
  system: "跟随系统代理",
  direct: "直连（不走代理）",
  custom: "自定义代理",
};

const DEFAULT_TARGET = "www.google.com:443";

function emptyForm(): FormState {
  return {
    id: null,
    label: "",
    kind: "socks5",
    host: "127.0.0.1",
    port: 1080,
    username: "",
    password: "",
    clearPassword: false,
  };
}

function formFromProxy(proxy: Proxy): FormState {
  return {
    id: proxy.id,
    label: proxy.label,
    kind: proxy.kind,
    host: proxy.host,
    port: proxy.port,
    username: proxy.username,
    password: "",
    clearPassword: false,
  };
}

function toConfig(form: FormState): ProxyConfig {
  return {
    id: form.id === null ? undefined : form.id,
    label: form.label.trim(),
    kind: form.kind,
    host: form.host.trim(),
    port: form.port,
    username: form.username.trim(),
  };
}

function validate(form: FormState): string | null {
  if (form.host.trim() === "") return "请填写代理服务器地址";
  if (form.port < 1 || form.port > 65535) return "代理端口要在 1 到 65535 之间";
  if (form.username.trim() !== "" && form.password.trim() === "" && form.id === null) {
    return "填了代理登录名就要同时填密码";
  }
  if (form.clearPassword && form.id === null) return "新建代理时没有「清空密码」这项";
  return null;
}

interface Props {
  /** 通知外壳：代理列表变了，账号面板里的「指定代理」选项要重新拉取。 */
  onChanged: () => void;
}

export default function ProxyPanel({ onChanged }: Props) {
  const [proxies, setProxies] = useState<Proxy[] | null>(null);
  const [global, setGlobal] = useState<GlobalProxy | null>(null);
  const [globalDraft, setGlobalDraft] = useState<GlobalProxyMode>("system");
  const [globalProxyId, setGlobalProxyId] = useState("");
  const [form, setForm] = useState<FormState | null>(null);
  const [target, setTarget] = useState(DEFAULT_TARGET);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [list, settings] = await Promise.all([api.listProxies(), api.getProxySettings()]);
      setProxies(list);
      setGlobal(settings);
      setGlobalDraft(settings.mode);
      setGlobalProxyId(settings.proxyId === undefined ? "" : String(settings.proxyId));
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  function applyGlobal(settings: GlobalProxy) {
    setGlobal(settings);
    setGlobalDraft(settings.mode);
    setGlobalProxyId(settings.proxyId === undefined ? "" : String(settings.proxyId));
  }

  function openCreate() {
    setForm(emptyForm());
    setError(null);
    setNotice(null);
  }

  function openEdit(proxy: Proxy) {
    setForm(formFromProxy(proxy));
    setError(null);
    setNotice(null);
  }

  function patch(change: Partial<FormState>) {
    setForm((current) => (current === null ? null : { ...current, ...change }));
    setNotice(null);
  }

  async function handleSave() {
    if (form === null) return;
    const problem = validate(form);
    if (problem) {
      setError(problem);
      return;
    }
    setBusy("save");
    setError(null);
    setNotice(null);
    try {
      const password =
        form.id === null
          ? form.password === ""
            ? undefined
            : form.password
          : form.clearPassword
            ? ""
            : form.password === ""
              ? undefined
              : form.password;
      const saved = await api.saveProxy(toConfig(form), password);
      setNotice(`代理「${saved.label || `${saved.host}:${saved.port}`}」已保存。`);
      setForm(null);
      await reload();
      onChanged();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleDelete(proxy: Proxy) {
    const name = proxy.label || `${proxy.host}:${proxy.port}`;
    if (!window.confirm(`确定删除代理「${name}」吗？引用它的账号会自动改回「跟随全局」。`)) {
      return;
    }
    setBusy(`delete-${proxy.id}`);
    setError(null);
    setNotice(null);
    try {
      await api.deleteProxy(proxy.id);
      setNotice(`已删除代理「${name}」。`);
      await reload();
      onChanged();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleTest(proxy: Proxy) {
    const name = proxy.label || `${proxy.host}:${proxy.port}`;
    setBusy(`test-${proxy.id}`);
    setError(null);
    setNotice(null);
    try {
      await api.testProxy(proxy.id, target.trim() === "" ? undefined : target.trim());
      setNotice(`代理「${name}」测试通过：已经能建立到 ${target.trim() || DEFAULT_TARGET} 的连接。`);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleSaveGlobal() {
    if (globalDraft === "custom" && globalProxyId === "") {
      setError("全局选了「自定义代理」就要挑一个具体代理");
      return;
    }
    setBusy("global");
    setError(null);
    setNotice(null);
    try {
      const mode: GlobalProxy =
        globalDraft === "custom"
          ? { mode: "custom", proxyId: Number(globalProxyId) }
          : { mode: globalDraft };
      const saved = await api.setProxySettings(mode);
      applyGlobal(saved);
      setNotice("全局代理策略已保存。");
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="panel" aria-busy={proxies === null || busy !== null}>
      <div className="panel-head">
        <h2>代理设置</h2>
        {form === null && (
          <button type="button" onClick={openCreate}>
            新增代理
          </button>
        )}
      </div>

      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}

      <div className="global-row">
        <label>
          全局策略
          <select
            value={globalDraft}
            onChange={(event) => {
              setGlobalDraft(event.target.value as GlobalProxyMode);
              setNotice(null);
            }}
          >
            {(Object.keys(GLOBAL_MODE_LABELS) as GlobalProxyMode[]).map((value) => (
              <option key={value} value={value}>
                {GLOBAL_MODE_LABELS[value]}
              </option>
            ))}
          </select>
        </label>
        {globalDraft === "custom" && (
          <label>
            用哪个代理
            <select value={globalProxyId} onChange={(event) => setGlobalProxyId(event.target.value)}>
              <option value="">请选择</option>
              {(proxies ?? []).map((proxy) => (
                <option key={proxy.id} value={String(proxy.id)}>
                  {proxy.label || `${proxy.host}:${proxy.port}`}
                </option>
              ))}
            </select>
          </label>
        )}
        <button type="button" aria-busy={busy === "global"} onClick={() => void handleSaveGlobal()} disabled={busy !== null}>
          {busy === "global" ? "保存中……" : "保存全局策略"}
        </button>
        {global && (
          <span className="hint" role="status">
            当前生效：{GLOBAL_MODE_LABELS[global.mode]}
            {global.mode === "custom" ? `（#${global.proxyId ?? "?"}）` : ""}
          </span>
        )}
      </div>

      <div className="global-row">
        <label className="target-field">
          测试目标
          <input
            value={target}
            onChange={(event) => setTarget(event.target.value)}
            placeholder={DEFAULT_TARGET}
          />
        </label>
        <span className="hint">点每个代理后面的「测试」按钮，就用这个目标去连一次。</span>
      </div>

      {proxies === null ? (
        <p className="hint" role="status">正在读取代理……</p>
      ) : proxies.length === 0 ? (
        <p className="hint">还没有自定义代理。全局策略可以先选「跟随系统」或「直连」。</p>
      ) : (
        <ul className="card-list">
          {proxies.map((proxy) => (
            <li key={proxy.id} className="card">
              <div className="card-main">
                <div className="card-title">
                  {proxy.label || `${proxy.host}:${proxy.port}`}
                  <span className="tag">{PROXY_KIND_LABELS[proxy.kind]}</span>
                  {proxy.username !== "" && (
                    <span className="tag">{proxy.hasPassword ? "带密码" : "缺密码"}</span>
                  )}
                </div>
                <div className="card-sub path">
                  {proxy.host}:{proxy.port}
                  {proxy.username !== "" ? ` · 登录名 ${proxy.username}` : ""}
                </div>
              </div>
              <div className="card-actions">
                <button type="button" onClick={() => openEdit(proxy)} disabled={busy !== null}>
                  编辑
                </button>
                <button
                  type="button"
                  onClick={() => void handleTest(proxy)}
                  aria-busy={busy === `test-${proxy.id}`}
                  disabled={busy !== null}
                >
                  {busy === `test-${proxy.id}` ? "测试中……" : "测试"}
                </button>
                <button
                  type="button"
                  className="danger"
                  onClick={() => void handleDelete(proxy)}
                  aria-busy={busy === `delete-${proxy.id}`}
                  disabled={busy !== null}
                >
                  {busy === `delete-${proxy.id}` ? "删除中……" : "删除"}
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
          <h3>{form.id === null ? "新增代理" : `编辑代理 #${form.id}`}</h3>

          <div className="field-row">
            <label>
              名字
              <input
                value={form.label}
                onChange={(event) => patch({ label: event.target.value })}
                placeholder="例如：公司代理"
              />
            </label>
            <label>
              类型
              <select
                value={form.kind}
                onChange={(event) => patch({ kind: event.target.value as ProxyKind })}
              >
                {(Object.keys(PROXY_KIND_LABELS) as ProxyKind[]).map((value) => (
                  <option key={value} value={value}>
                    {PROXY_KIND_LABELS[value]}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <div className="field-row">
            <label>
              代理地址
              <input
                value={form.host}
                onChange={(event) => patch({ host: event.target.value })}
                placeholder="127.0.0.1"
              />
            </label>
            <label>
              端口
              <input
                type="number"
                min={1}
                max={65535}
                value={form.port}
                onChange={(event) => patch({ port: Number(event.target.value) })}
              />
            </label>
            <label>
              登录名（可选）
              <input
                value={form.username}
                onChange={(event) => patch({ username: event.target.value })}
                placeholder="没有认证就留空"
              />
            </label>
          </div>

          <label className="secret-field">
            {form.id === null ? "代理密码（可选）" : "新的代理密码"}
            <input
              type="password"
              autoComplete="off"
              value={form.password}
              disabled={form.clearPassword}
              onChange={(event) => patch({ password: event.target.value })}
              placeholder={
                form.id === null ? "只保存在 Windows 凭据管理器" : "留空表示不改，填新值表示替换"
              }
            />
          </label>
          {form.id !== null && (
            <label className="checkbox">
              <input
                type="checkbox"
                checked={form.clearPassword}
                onChange={(event) => patch({ clearPassword: event.target.checked, password: "" })}
              />
              清空已保存的密码（改成无认证代理）
            </label>
          )}

          <div className="form-actions">
            <button type="submit" className="primary" aria-busy={busy === "save"} disabled={busy !== null}>
              {busy === "save" ? "保存中……" : "保存"}
            </button>
            <button
              type="button"
              onClick={() => {
                setForm(null);
                setError(null);
              }}
              disabled={busy !== null}
            >
              取消
            </button>
          </div>
        </form>
      )}
    </section>
  );
}