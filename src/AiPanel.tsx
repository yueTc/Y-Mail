//! AI 站点与功能设置（Wave 7）。
//!
//! 安全约定：
//! - CDKey 只通过密码框传给后端，不写浏览器存储，也不在界面回显；
//! - 站点与模型列表只展示后端返回的非敏感字段；
//! - 模型输出一律当纯文本展示，绝不使用 dangerouslySetInnerHTML。

import { useCallback, useEffect, useState } from "react";

import {
  api,
  describeError,
  type AiAudit,
  type AiFunction,
  type AiModelMap,
  type AiProvider,
  type AiProviderDraft,
  type AiProviderKind,
  type AiThinkingLevel,
} from "./api";

/** 显示密钥图标：一只睁开的眼睛。 */
function EyeIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
        <circle cx="12" cy="12" r="3" />
      </g>
    </svg>
  );
}

/** 隐藏密钥图标：一只眼睛加斜杠。 */
function EyeOffIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M10.6 5.8A9.8 9.8 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16.4 16.4 0 0 1-3.2 4.2" />
        <path d="M6.2 6.9A16 16 0 0 0 2.5 12S6 18.5 12 18.5c1.6 0 3-.5 4.2-1.2" />
        <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
        <path d="M4.5 4.5 19.5 19.5" />
      </g>
    </svg>
  );
}

const FUNCTION_LABEL: Record<AiFunction, string> = {
  translate: "翻译",
  summary: "摘要",
  polish: "润色",
  draft: "起草",
};

const FUNCTION_ORDER: AiFunction[] = ["translate", "summary", "polish", "draft"];

const KIND_LABEL: Record<AiProviderKind, string> = {
  openai_compatible: "OpenAI 兼容站点",
  deepl: "DeepL",
  ollama: "本机 Ollama",
};

const THINKING_LABEL: Record<AiThinkingLevel, string> = {
  off: "关闭",
  low: "低",
  medium: "中",
  high: "高",
};

const THINKING_ORDER: AiThinkingLevel[] = ["off", "low", "medium", "high"];

interface ProviderDraftState {
  id?: number;
  label: string;
  kind: AiProviderKind;
  baseUrl: string;
  defaultModel: string;
  modelsText: string;
  thinkingLevel: AiThinkingLevel;
  enabled: boolean;
}

interface FeatureDraftState {
  providerId: string;
  model: string;
  thinkingLevel: "" | AiThinkingLevel;
}

function emptyProvider(): ProviderDraftState {
  return {
    label: "",
    kind: "openai_compatible",
    baseUrl: "",
    defaultModel: "",
    modelsText: "",
    thinkingLevel: "off",
    enabled: false,
  };
}

function providerToDraft(provider: AiProvider): ProviderDraftState {
  return {
    id: provider.id,
    label: provider.label,
    kind: provider.kind,
    baseUrl: provider.baseUrl,
    defaultModel: provider.defaultModel,
    modelsText: provider.models.join("\n"),
    thinkingLevel: provider.thinkingLevel,
    enabled: provider.enabled,
  };
}

function parseModels(text: string): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const model = line.trim();
    if (model === "" || seen.has(model)) continue;
    seen.add(model);
    result.push(model);
  }
  return result;
}

function featureMapFor(maps: AiModelMap[], fn: AiFunction): AiModelMap | undefined {
  return maps.find((item) => item.function === fn);
}

/** AI 设置页：站点管理、功能级配置、审计与熔断。 */
export default function AiPanel() {
  const [providers, setProviders] = useState<AiProvider[]>([]);
  const [audits, setAudits] = useState<AiAudit[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [editing, setEditing] = useState<ProviderDraftState>();
  const [apiKey, setApiKey] = useState("");
  const [showApiKey, setShowApiKey] = useState(false);
  const [modelsHint, setModelsHint] = useState("");
  const [featureDrafts, setFeatureDrafts] = useState<Record<string, FeatureDraftState>>({});

  const refresh = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [nextProviders, nextMaps, nextAudits] = await Promise.all([
        api.listAiProviders(),
        api.listAiModelMaps(),
        api.listAiAudit(50),
      ]);
      setProviders(nextProviders);
      setAudits(nextAudits);
      setFeatureDrafts((old) => {
        const next: Record<string, FeatureDraftState> = {};
        for (const fn of FUNCTION_ORDER) {
          const existing = old[fn];
          const map = featureMapFor(nextMaps, fn);
          next[fn] = existing ?? {
            providerId: map ? String(map.providerId) : "",
            model: map?.model ?? "",
            thinkingLevel: map?.thinkingLevel ?? "",
          };
        }
        return next;
      });
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const startCreate = useCallback(() => {
    setEditing(emptyProvider());
    setApiKey("");
    setShowApiKey(false);
    setModelsHint("");
    setNotice("");
    setError("");
  }, []);

  const startEdit = useCallback((provider: AiProvider) => {
    setEditing(providerToDraft(provider));
    setApiKey("");
    setShowApiKey(false);
    setModelsHint("");
    setNotice("");
    setError("");
  }, []);

  const saveProvider = useCallback(async () => {
    if (!editing) return;
    if (editing.label.trim() === "") {
      setError("请填写站点名称");
      return;
    }
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const draft: AiProviderDraft = {
        ...(editing.id === undefined ? {} : { id: editing.id }),
        label: editing.label,
        kind: editing.kind,
        baseUrl: editing.baseUrl,
        defaultModel: editing.defaultModel,
        models: parseModels(editing.modelsText),
        thinkingLevel: editing.thinkingLevel,
        enabled: editing.enabled,
      };
      const saved = await api.saveAiProvider(
        draft,
        editing.id === undefined ? apiKey : apiKey === "" ? undefined : apiKey,
      );
      setApiKey("");
      setEditing(undefined);
      setNotice(`已保存「${saved.label}」`);
      await refresh();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [apiKey, editing, refresh]);

  const testDraft = useCallback(async () => {
    if (!editing) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const models = await api.testAiProvider(
        editing.kind,
        editing.baseUrl,
        apiKey === "" ? undefined : apiKey,
        editing.id,
      );
      setModelsHint(`连接成功，拉到 ${models.length} 个模型。`);
      if (models.length > 0) {
        setEditing((old) => (old ? { ...old, modelsText: models.join("\n") } : old));
      }
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [apiKey, editing]);

  const refreshModels = useCallback(
    async (id: number) => {
      setBusy(true);
      setError("");
      setNotice("");
      try {
        const models = await api.refreshAiProviderModels(id);
        setNotice(`已拉取 ${models.length} 个模型`);
        await refresh();
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const removeProvider = useCallback(
    async (provider: AiProvider) => {
      if (!window.confirm(`确定删除 AI 站点「${provider.label}」吗？相关缓存也会清理。`)) return;
      setBusy(true);
      setError("");
      setNotice("");
      try {
        await api.deleteAiProvider(provider.id);
        setNotice(`已删除「${provider.label}」`);
        if (editing?.id === provider.id) setEditing(undefined);
        await refresh();
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [editing?.id, refresh],
  );

  const saveFeature = useCallback(
    async (fn: AiFunction) => {
      const draft = featureDrafts[fn];
      if (!draft?.providerId) {
        setError(`请先为「${FUNCTION_LABEL[fn]}」选择站点`);
        return;
      }
      setBusy(true);
      setError("");
      setNotice("");
      try {
        await api.setAiFeature(
          fn,
          Number(draft.providerId),
          draft.model,
          draft.thinkingLevel === "" ? undefined : draft.thinkingLevel,
        );
        setNotice(`已保存「${FUNCTION_LABEL[fn]}」的模型设置`);
        await refresh();
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [featureDrafts, refresh],
  );

  const clearFeature = useCallback(
    async (fn: AiFunction) => {
      setBusy(true);
      setError("");
      setNotice("");
      try {
        await api.clearAiFeature(fn);
        setFeatureDrafts((old) => ({
          ...old,
          [fn]: { providerId: "", model: "", thinkingLevel: "" },
        }));
        setNotice(`已清除「${FUNCTION_LABEL[fn]}」的单独设置，将回退站点默认`);
        await refresh();
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const disableAll = useCallback(async () => {
    if (!window.confirm("确定一键关闭 AI 吗？所有站点会停用，已发出的授权也会作废。")) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const count = await api.disableAllAi();
      setNotice(`已关闭 AI，停用 ${count} 个站点`);
      await refresh();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const clearCache = useCallback(async () => {
    if (!window.confirm("确定清空 AI 缓存吗？清空后翻译和摘要会重新请求模型。")) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const count = await api.clearAiCache();
      setNotice(`已清空 ${count} 条 AI 缓存`);
      await refresh();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  return (
    <section className="panel ai-panel" aria-busy={loading || busy}>
      <div className="panel-head">
        <div>
          <h2>AI 与翻译</h2>
          <p className="hint">
            默认关闭。Key 只进系统保险箱；非本机地址必须用 HTTPS。每次真正外发前都会弹窗确认域名、模型和是否本地。
          </p>
        </div>
        <div className="ai-panel-actions">
          <button type="button" className="danger" onClick={() => void disableAll()} disabled={busy}>
            一键关闭 AI
          </button>
          <button type="button" onClick={() => void clearCache()} disabled={busy}>
            清空缓存
          </button>
        </div>
      </div>

      {loading && <p className="hint" role="status">正在读取 AI 设置……</p>}
      {error && <p className="error" role="alert">操作失败：{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}

      <h3>AI 站点</h3>
      <div className="ai-provider-list">
        {providers.length === 0 && !loading && (
          <p className="hint">还没有站点。添加一个后，AI 功能才会出现可用状态。</p>
        )}
        {providers.map((provider) => (
          <article key={provider.id} className="ai-provider-card">
            <div className="ai-provider-main">
              <div className="ai-provider-title">
                <strong>{provider.label}</strong>
                <span className={provider.enabled ? "tag ai-enabled" : "tag"}>
                  {provider.enabled ? "已启用" : "已停用"}
                </span>
                <span className="tag">{KIND_LABEL[provider.kind] ?? provider.kind}</span>
                <span className="tag">{provider.hasKey ? "已有密钥" : "无密钥"}</span>
              </div>
              <div className="card-sub">地址：{provider.baseUrl}</div>
              <div className="card-sub">
                默认模型：{provider.defaultModel || "未填写"}；思考程度：
                {THINKING_LABEL[provider.thinkingLevel]}
              </div>
              <div className="card-sub">
                模型：{provider.models.length > 0 ? provider.models.join("、") : "未拉取，可手工填写"}
              </div>
            </div>
            <div className="card-actions">
              <button type="button" onClick={() => startEdit(provider)} disabled={busy}>
                编辑
              </button>
              <button type="button" onClick={() => void refreshModels(provider.id)} disabled={busy}>
                拉取模型
              </button>
              <button type="button" className="danger" onClick={() => void removeProvider(provider)} disabled={busy}>
                删除
              </button>
            </div>
          </article>
        ))}
      </div>

      {!editing && (
        <button type="button" className="ai-add-provider" onClick={startCreate} disabled={busy}>
          新增 AI 站点
        </button>
      )}

      {editing && (
        <div className="form ai-provider-form">
          <h3>{editing.id === undefined ? "新增 AI 站点" : "编辑 AI 站点"}</h3>
          <div className="field-row">
            <label>
              站点名称
              <input
                aria-label="AI 站点名称"
                value={editing.label}
                onChange={(event) =>
                  setEditing((old) => (old ? { ...old, label: event.target.value } : old))
                }
              />
            </label>
            <label>
              站点类型
              <select
                aria-label="AI 站点类型"
                value={editing.kind}
                onChange={(event) =>
                  setEditing((old) =>
                    old ? { ...old, kind: event.target.value as AiProviderKind } : old,
                  )
                }
              >
                <option value="openai_compatible">OpenAI 兼容站点</option>
                <option value="deepl">DeepL</option>
                <option value="ollama">本机 Ollama</option>
              </select>
            </label>
          </div>
          <label className="secret-field">
            站点地址
            <input
              aria-label="AI 站点地址"
              value={editing.baseUrl}
              placeholder="例如 https://api.example.com/v1，本机 Ollama 可填 http://127.0.0.1:11434"
              onChange={(event) =>
                setEditing((old) => (old ? { ...old, baseUrl: event.target.value } : old))
              }
            />
          </label>
          <label className="secret-field">
            CDKey / API Key
            <span className="secret-input-row">
              <input
                type={showApiKey ? "text" : "password"}
                aria-label="CDKey / API Key"
                autoComplete="off"
                value={apiKey}
                placeholder={editing.id === undefined ? "新建时填写" : "留空沿用已存密钥，输入后替换"}
                onChange={(event) => setApiKey(event.target.value)}
              />
              <button
                type="button"
                className="secret-toggle"
                aria-label={showApiKey ? "隐藏密钥" : "显示密钥"}
                title={showApiKey ? "隐藏密钥" : "显示密钥"}
                aria-pressed={showApiKey}
                onClick={() => setShowApiKey((old) => !old)}
              >
                {showApiKey ? <EyeOffIcon /> : <EyeIcon />}
              </button>
            </span>
          </label>
          <div className="field-row">
            <label>
              默认模型
              <input
                aria-label="默认模型"
                value={editing.defaultModel}
                placeholder="不填写就使用功能级模型"
                onChange={(event) =>
                  setEditing((old) => (old ? { ...old, defaultModel: event.target.value } : old))
                }
              />
            </label>
            <label>
              默认思考程度
              <select
                aria-label="默认思考程度"
                value={editing.thinkingLevel}
                onChange={(event) =>
                  setEditing((old) =>
                    old ? { ...old, thinkingLevel: event.target.value as AiThinkingLevel } : old,
                  )
                }
              >
                {THINKING_ORDER.map((level) => (
                  <option key={level} value={level}>
                    {THINKING_LABEL[level]}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <label className="secret-field">
            模型列表（每行一个，也可手工填写）
            <textarea
              aria-label="模型列表"
              rows={4}
              value={editing.modelsText}
              onChange={(event) =>
                setEditing((old) => (old ? { ...old, modelsText: event.target.value } : old))
              }
            />
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={editing.enabled}
              onChange={(event) =>
                setEditing((old) => (old ? { ...old, enabled: event.target.checked } : old))
              }
            />
            启用这个站点
          </label>
          {modelsHint && <p className="hint" role="status">{modelsHint}</p>}
          <div className="form-actions">
            <button type="button" className="primary" onClick={() => void saveProvider()} disabled={busy}>
              保存
            </button>
            <button type="button" onClick={() => void testDraft()} disabled={busy}>
              测试连接并拉取模型
            </button>
            <button type="button" onClick={() => setEditing(undefined)} disabled={busy}>
              取消
            </button>
          </div>
        </div>
      )}

      <h3>功能级模型与思考程度</h3>
      <p className="hint">没有单独指定时回退到站点默认模型；思考程度不支持时后端会自动降级并在结果里标注。</p>
      <div className="ai-feature-list">
        {FUNCTION_ORDER.map((fn) => {
          const draft = featureDrafts[fn] ?? { providerId: "", model: "", thinkingLevel: "" };
          const selected = providers.find((provider) => String(provider.id) === draft.providerId);
          return (
            <section key={fn} className="ai-feature-card">
              <h4>{FUNCTION_LABEL[fn]}</h4>
              <label className="secret-field">
                站点
                <select
                  aria-label={`${FUNCTION_LABEL[fn]}站点`}
                  value={draft.providerId}
                  onChange={(event) =>
                    setFeatureDrafts((old) => ({
                      ...old,
                      [fn]: { ...draft, providerId: event.target.value, model: "" },
                    }))
                  }
                >
                  <option value="">自动选择第一个启用站点</option>
                  {providers.map((provider) => (
                    <option key={provider.id} value={provider.id}>
                      {provider.label}{provider.enabled ? "" : "（已停用）"}
                    </option>
                  ))}
                </select>
              </label>
              <label className="secret-field">
                模型
                <input
                  aria-label={`${FUNCTION_LABEL[fn]}模型`}
                  list={`ai-models-${fn}`}
                  value={draft.model}
                  placeholder={selected?.defaultModel || "留空回退站点默认模型"}
                  onChange={(event) =>
                    setFeatureDrafts((old) => ({ ...old, [fn]: { ...draft, model: event.target.value } }))
                  }
                />
              </label>
              <datalist id={`ai-models-${fn}`}>
                {(selected?.models ?? []).map((model) => (
                  <option key={model} value={model} />
                ))}
              </datalist>
              <label className="secret-field">
                思考程度
                <select
                  aria-label={`${FUNCTION_LABEL[fn]}思考程度`}
                  value={draft.thinkingLevel}
                  onChange={(event) =>
                    setFeatureDrafts((old) => ({
                      ...old,
                      [fn]: {
                        ...draft,
                        thinkingLevel: event.target.value as "" | AiThinkingLevel,
                      },
                    }))
                  }
                >
                  <option value="">回退站点默认</option>
                  {THINKING_ORDER.map((level) => (
                    <option key={level} value={level}>
                      {THINKING_LABEL[level]}
                    </option>
                  ))}
                </select>
              </label>
              <div className="form-actions">
                <button type="button" onClick={() => void saveFeature(fn)} disabled={busy}>
                  保存设置
                </button>
                <button type="button" onClick={() => void clearFeature(fn)} disabled={busy}>
                  恢复默认
                </button>
              </div>
            </section>
          );
        })}
      </div>

      <h3>AI 调用审计</h3>
      <p className="hint">只记录时间、功能、站点、模型、是否外发和结果，不记录邮件正文或密钥。</p>
      {audits.length === 0 ? (
        <p className="hint">还没有 AI 调用记录。</p>
      ) : (
        <div className="ai-audit-wrap">
          <table className="ai-audit">
            <thead>
              <tr>
                <th>时间</th>
                <th>功能</th>
                <th>站点</th>
                <th>模型</th>
                <th>目标域名</th>
                <th>是否本地</th>
                <th>结果</th>
              </tr>
            </thead>
            <tbody>
              {audits.map((audit) => (
                <tr key={audit.id}>
                  <td>{audit.createdAt}</td>
                  <td>{FUNCTION_LABEL[audit.function as AiFunction] ?? audit.function}</td>
                  <td>{audit.providerLabel}</td>
                  <td>{audit.model}</td>
                  <td>{audit.targetHost}</td>
                  <td>{audit.local ? "本地" : "外发"}</td>
                  <td>{audit.outcome}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}