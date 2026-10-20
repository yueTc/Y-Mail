//! AI 站点设置弹窗：新增 / 编辑共用，交互照抄账号弹窗。
//!
//! 安全约定：
//! - CDKey 只通过密码框传给后端，不写浏览器存储，也不在界面回显；
//! - 表单只展示后端返回的非敏感字段；代理密码不进这个组件。

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type JSX,
} from "react";

import { PROXY_MODE_LABELS } from "./AccountForm";
import {
  api,
  describeError,
  type AccountProxy,
  type AccountProxyMode,
  type AiProvider,
  type AiProviderDraft,
  type AiProviderKind,
  type AiThinkingLevel,
  type Proxy,
} from "./api";
import { t } from "./i18n";
import AiTestModelPicker from "./AiTestModelPicker";

/** 思考程度四档的中文名；站点卡片列站点时也用同一份。 */
export const THINKING_LABEL: Record<AiThinkingLevel, string> = {
  off: "关闭",
  low: "低",
  medium: "中",
  high: "高",
};

export const THINKING_ORDER: AiThinkingLevel[] = ["off", "low", "medium", "high"];

/** 需要选测试模型的决策站点：通用决策模型和旧 TypeSafe。 */
export function isDecisionKind(kind: AiProviderKind): boolean {
  return kind === "decision_api" || kind === "typesafe";
}

/** 站点类型的中文名；站点卡片列站点时也用同一份。 */
export const KIND_LABEL: Record<AiProviderKind, string> = {
  openai_compatible: "OpenAI 兼容站点",
  deepl: "DeepL",
  ollama: "本机 Ollama",
  typesafe: "旧版 TypeSafe jev（兼容）",
  decision_api: "决策模型站点",
};

export type AiProviderDialogProps = {
  open: boolean;
  /** 要编辑的站点；传 null 表示新增。 */
  provider: AiProvider | null;
  onClose: () => void;
  onSaved: (saved: AiProvider) => void;
};

interface ProviderDraftState {
  id?: number;
  label: string;
  kind: AiProviderKind;
  baseUrl: string;
  defaultModel: string;
  modelsText: string;
  thinkingLevel: AiThinkingLevel;
  enabled: boolean;
  proxyMode: AccountProxyMode;
  proxyId: string;
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
    proxyMode: "inherit",
    proxyId: "",
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
    proxyMode: provider.proxy.mode,
    proxyId: provider.proxy.proxyId === undefined ? "" : String(provider.proxy.proxyId),
  };
}

/** 模型列表按行拆分，去空行、去重复。 */
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

/** 把表单里的代理策略转成后端入参；「指定代理」没挑代理时返回 null。 */
function proxyForDraft(draft: ProviderDraftState): AccountProxy | null {
  if (draft.proxyMode !== "custom") return { mode: draft.proxyMode };
  const id = Number(draft.proxyId);
  if (draft.proxyId === "" || !Number.isFinite(id)) return null;
  return { mode: "custom", proxyId: id };
}

/** 代理下拉里的显示名：有名字带名字，再补类型与地址。 */
function proxyLabel(proxy: Proxy): string {
  const name = proxy.label.trim() === "" ? `${proxy.host}:${proxy.port}` : proxy.label;
  return `${name}（${proxy.kind} ${proxy.host}:${proxy.port}）`;
}

const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** 弹窗里所有能按到的元素；浏览器里优先取真正可见的，兜底再退回全部。 */
function focusableWithin(root: HTMLElement | null): HTMLElement[] {
  if (root === null) return [];
  const all = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
  const visible = all.filter((element) => element.offsetParent !== null);
  return visible.length > 0 ? visible : all;
}

/** 打开弹窗后焦点要落在第一个输入项，而不是标题或关闭按钮。 */
function firstField(root: HTMLElement | null): HTMLElement | null {
  if (root === null) return null;
  return root.querySelector<HTMLElement>(
    "input:not([disabled]), select:not([disabled]), textarea:not([disabled])",
  );
}

const BACKDROP_STYLE: CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(15, 23, 42, 0.45)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: 16,
  zIndex: 1000,
};

const DIALOG_STYLE: CSSProperties = {
  width: "min(760px, 90vw)",
  maxHeight: "90vh",
  display: "flex",
  flexDirection: "column",
  background: "var(--surface, #ffffff)",
  color: "inherit",
  borderRadius: 12,
  boxShadow: "0 24px 60px rgba(15, 23, 42, 0.35)",
  overflow: "hidden",
};

const HEADER_STYLE: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 12,
  padding: "16px 20px",
  borderBottom: "1px solid rgba(148, 163, 184, 0.35)",
};

const BODY_STYLE: CSSProperties = {
  padding: "16px 20px 20px",
  overflowY: "auto",
};

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

export default function AiProviderDialog({
  open,
  provider,
  onClose,
  onSaved,
}: AiProviderDialogProps): JSX.Element {
  const editingProvider = provider !== null;
  const [editing, setEditing] = useState<ProviderDraftState>(() => emptyProvider());
  const [apiKey, setApiKey] = useState("");
  const [showApiKey, setShowApiKey] = useState(false);
  const [modelsHint, setModelsHint] = useState("");
  const [testModel, setTestModel] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [proxies, setProxies] = useState<Proxy[]>([]);
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const initialRef = useRef("");
  const dirtyRef = useRef(false);
  const busyRef = useRef(false);

  // 打开时重置表单：新增用空草稿，编辑回填已存站点；同时拉一次可选代理。
  useEffect(() => {
    if (!open) return;
    const next = provider === null ? emptyProvider() : providerToDraft(provider);
    setEditing(next);
    setApiKey("");
    setShowApiKey(false);
    setModelsHint("");
    setTestModel(null);
    setError("");
    initialRef.current = JSON.stringify(next);
    dirtyRef.current = false;
    void api
      .listProxies()
      .then((items) => setProxies(items))
      .catch(() => setProxies([]));
  }, [open, provider]);

  // 打开时记住是谁打开的、把焦点送进第一个输入框；关闭时把焦点还回去。
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    restoreFocusRef.current = previous instanceof HTMLElement ? previous : null;
    const timer = window.setTimeout(() => {
      firstField(dialogRef.current)?.focus();
    }, 0);
    return () => {
      window.clearTimeout(timer);
      const target = restoreFocusRef.current;
      if (target !== null && document.contains(target)) target.focus();
    };
  }, [open]);

  // 有改动才算脏；密钥栏只要填了就当作脏。
  useEffect(() => {
    if (!open) return;
    dirtyRef.current = apiKey !== "" || JSON.stringify(editing) !== initialRef.current;
  }, [open, editing, apiKey]);

  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);

  const requestClose = useCallback(() => {
    // 正在测试 / 保存时先别关，避免半截请求把状态搞乱。
    if (busyRef.current) return;
    if (dirtyRef.current && !window.confirm(t("表单里还有没保存的内容，确定放弃吗？"))) {
      return;
    }
    onClose();
  }, [onClose]);

  // Esc 关闭；Tab 在弹窗内循环，别跑到后面的页面上去。
  useEffect(() => {
    if (!open) return;
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        requestClose();
        return;
      }
      if (event.key !== "Tab") return;
      const root = dialogRef.current;
      if (root === null) return;
      const items = focusableWithin(root);
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      const inside = active instanceof HTMLElement && root.contains(active);
      if (event.shiftKey) {
        if (!inside || active === first) {
          event.preventDefault();
          last.focus();
        }
      } else if (!inside || active === last) {
        event.preventDefault();
        first.focus();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [open, requestClose]);

  const saveProvider = useCallback(async () => {
    if (editing.label.trim() === "") {
      setError(t("请填写站点名称"));
      return;
    }
    const proxy = proxyForDraft(editing);
    if (proxy === null) {
      setError(t("选了「指定代理」就要挑一个具体代理"));
      return;
    }
    setBusy(true);
    setError("");
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
        proxy,
      };
      const saved = await api.saveAiProvider(
        draft,
        editing.id === undefined ? apiKey : apiKey === "" ? undefined : apiKey,
      );
      onSaved(saved);
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [apiKey, editing, onSaved]);

  const selectedTestModel =
    testModel ?? (editing.defaultModel.trim() || parseModels(editing.modelsText)[0] || "");

  const testDraft = useCallback(async () => {
    setModelsHint("");
    if (isDecisionKind(editing.kind) && selectedTestModel.trim() === "") {
      setError(t("请选择或填写测试模型"));
      return;
    }
    const proxy = proxyForDraft(editing);
    if (proxy === null) {
      setError(t("选了「指定代理」就要挑一个具体代理"));
      return;
    }
    setBusy(true);
    setError("");
    try {
      const models = await api.testAiProvider(
        editing.kind,
        editing.baseUrl,
        apiKey === "" ? undefined : apiKey,
        editing.id,
        proxy,
        isDecisionKind(editing.kind) ? selectedTestModel.trim() : undefined,
      );
      setModelsHint(
        isDecisionKind(editing.kind)
          ? t("连接成功，测试模型：{0}", [selectedTestModel.trim()])
          : t("连接成功，拉到 {0} 个模型。", [models.length]),
      );
      if (!isDecisionKind(editing.kind) && models.length > 0) {
        setEditing((old) => ({ ...old, modelsText: models.join("\n") }));
      }
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [apiKey, editing, selectedTestModel]);

  const baseUrlFieldLabel = editing.kind === "decision_api" ? "完整接口地址" : "站点地址";
  const decisionAddressHint =
    "填写完整请求地址，例如 https://api.bamboonode.dev/v1/decisions 或 https://api.typesafe.ai/v1/systemone";

  // 关着的时候渲染一个空片段：既满足 JSX.Element 返回类型，也不会挂载表单。
  if (!open) return <></>;

  return (
    <div
      style={BACKDROP_STYLE}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="ai-provider-dialog-title"
        style={DIALOG_STYLE}
      >
        <header style={HEADER_STYLE}>
          <h2 id="ai-provider-dialog-title" style={{ margin: 0, fontSize: 16 }}>
            {editingProvider ? t("编辑 AI 站点") : t("新增 AI 站点")}</h2>
          <button
            type="button"
            aria-label={t("关闭")}
            onClick={requestClose}
            style={{
              border: "none",
              background: "transparent",
              fontSize: 22,
              lineHeight: 1,
              cursor: "pointer",
              color: "inherit",
            }}
          >
            ×
          </button>
        </header>

        <div style={BODY_STYLE}>
          <div className="form ai-provider-form">
            {error && <p className="error" role="alert">{t("操作失败：")}{error}</p>}
            <div className="field-row">
              <label>
                {t("站点名称")}<input
                  aria-label={t("AI 站点名称")}
                  value={editing.label}
                  onChange={(event) => setEditing((old) => ({ ...old, label: event.target.value }))}
                />
              </label>
              <label>
                {t("站点类型")}<select
                  aria-label={t("AI 站点类型")}
                  value={editing.kind}
                  onChange={(event) => {
                    const kind = event.target.value as AiProviderKind;
                    setTestModel(null);
                    setModelsHint("");
                    setEditing((old) => {
                      // 决策模型站点都有官方默认值，空着就预填；旧 TypeSafe 继续兼容。
                      const preset =
                        kind === "decision_api"
                          ? {
                              baseUrl: "https://api.bamboonode.dev/v1/decisions",
                              defaultModel: "startlux-decision-27b-autothink",
                            }
                          : kind === "typesafe"
                            ? { baseUrl: "https://api.typesafe.ai/v1", defaultModel: "jev-latest" }
                            : undefined;
                      return {
                        ...old,
                        kind,
                        baseUrl:
                          preset && old.baseUrl.trim() === "" ? preset.baseUrl : old.baseUrl,
                        defaultModel:
                          preset && old.defaultModel.trim() === ""
                            ? preset.defaultModel
                            : old.defaultModel,
                      };
                    });
                  }}
                >
                  {(Object.keys(KIND_LABEL) as AiProviderKind[])
                    .filter((kind) => kind !== "typesafe" || provider?.kind === "typesafe")
                    .map((kind) => (
                      <option key={kind} value={kind}>
                        {t(KIND_LABEL[kind])}
                      </option>
                    ))}
                </select>
              </label>
            </div>
            <label className="secret-field" htmlFor="ai-provider-base-url">
              {t(baseUrlFieldLabel)}<input
                id="ai-provider-base-url"
                value={editing.baseUrl}
                aria-describedby={
                  editing.kind === "decision_api" ? "ai-provider-base-url-hint" : undefined
                }
                placeholder={
                  editing.kind === "decision_api"
                    ? t(decisionAddressHint)
                    : editing.kind === "typesafe"
                      ? t("填到 /v1 为止，例如 https://api.typesafe.ai/v1，不要带 /systemone")
                      : t("例如 https://api.example.com/v1，本机 Ollama 可填 http://127.0.0.1:11434")
                }
                onChange={(event) => setEditing((old) => ({ ...old, baseUrl: event.target.value }))}
              />
            </label>
            {editing.kind === "decision_api" && (
              <p id="ai-provider-base-url-hint" className="hint">
                {t(decisionAddressHint)}
              </p>
            )}
            <label className="secret-field">
              CDKey / API Key
              <span className="secret-input-row">
                <input
                  className="secret-input"
                  type={showApiKey ? "text" : "password"}
                  aria-label="CDKey / API Key"
                  autoComplete="off"
                  value={apiKey}
                  placeholder={
                    editing.id === undefined ? t("新建时填写") : t("留空沿用已存密钥，输入后替换")
                  }
                  onChange={(event) => setApiKey(event.target.value)}
                />
                <button
                  type="button"
                  className="secret-toggle"
                  aria-label={showApiKey ? t("隐藏密钥") : t("显示密钥")}
                  title={showApiKey ? t("隐藏密钥") : t("显示密钥")}
                  aria-pressed={showApiKey}
                  onClick={() => setShowApiKey((old) => !old)}
                >
                  {showApiKey ? <EyeOffIcon /> : <EyeIcon />}
                </button>
              </span>
            </label>
            <div className="field-row">
              <label>
                {t("默认模型")}<input
                  aria-label={t("默认模型")}
                  value={editing.defaultModel}
                  placeholder={t("不填写就使用功能级模型")}
                  onChange={(event) =>
                    setEditing((old) => ({ ...old, defaultModel: event.target.value }))
                  }
                />
              </label>
              <label>
                {t("默认思考程度")}<select
                  aria-label={t("默认思考程度")}
                  value={editing.thinkingLevel}
                  onChange={(event) =>
                    setEditing((old) => ({
                      ...old,
                      thinkingLevel: event.target.value as AiThinkingLevel,
                    }))
                  }
                >
                  {THINKING_ORDER.map((level) => (
                    <option key={level} value={level}>
                      {t(THINKING_LABEL[level], undefined, "ai")}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <div className="field-row">
              <label>
                {t("代理策略")}<select
                  aria-label={t("AI 站点代理策略")}
                  value={editing.proxyMode}
                  onChange={(event) =>
                    setEditing((old) => ({
                      ...old,
                      proxyMode: event.target.value as AccountProxyMode,
                    }))
                  }
                >
                  {(Object.keys(PROXY_MODE_LABELS) as AccountProxyMode[]).map((mode) => (
                    <option key={mode} value={mode}>
                      {t(PROXY_MODE_LABELS[mode])}
                    </option>
                  ))}
                </select>
              </label>
              {editing.proxyMode === "custom" && (
                <label>
                  {t("指定代理")}<select
                    aria-label={t("AI 站点指定代理")}
                    value={editing.proxyId}
                    onChange={(event) =>
                      setEditing((old) => ({ ...old, proxyId: event.target.value }))
                    }
                  >
                    <option value="">{t("请选择")}</option>
                    {proxies.map((proxy) => (
                      <option key={proxy.id} value={String(proxy.id)}>
                        {proxyLabel(proxy)}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
            {editing.proxyMode === "custom" && proxies.length === 0 && (
              <p className="hint">{t("还没有自定义代理。先到「代理」里添加一个，再回来选。")}</p>
            )}
            <label className="secret-field">
              {t("模型列表（每行一个，也可手工填写）")}<textarea
                aria-label={t("模型列表")}
                rows={4}
                value={editing.modelsText}
                onChange={(event) =>
                  setEditing((old) => ({ ...old, modelsText: event.target.value }))
                }
              />
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={editing.enabled}
                onChange={(event) =>
                  setEditing((old) => ({ ...old, enabled: event.target.checked }))
                }
              />
              {t("启用这个站点")}</label>
            {isDecisionKind(editing.kind) && (
              <AiTestModelPicker
                value={selectedTestModel}
                defaultModel={editing.defaultModel}
                models={parseModels(editing.modelsText)}
                disabled={busy}
                onChange={(model) => {
                  setTestModel(model);
                  setModelsHint("");
                }}
              />
            )}
            {modelsHint && <p className="hint" role="status">{modelsHint}</p>}
            <div className="form-actions">
              <button type="button" className="primary" onClick={() => void saveProvider()} disabled={busy}>
                {t("保存")}</button>
              <button type="button" onClick={() => void testDraft()} disabled={busy}>
                {isDecisionKind(editing.kind) ? t("测试连接") : t("测试连接并拉取模型")}</button>
              <button type="button" onClick={requestClose} disabled={busy}>
                {t("取消")}</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}