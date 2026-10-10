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
  type MailLabel,
  type NotificationLatency,
} from "./api";
import { t } from "./i18n";

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
  notification_verify: "通知智能识别",
  classify: "邮件分类",
};

/** 逐次确认的功能；在功能级模型列表里平铺展示。 */
const FUNCTION_ORDER: AiFunction[] = ["translate", "summary", "polish", "draft", "classify"];

/** 通知智能识别单独放在「高级功能」区，跟普通功能分开显示。 */
const VERIFY_FUNCTION: AiFunction = "notification_verify";

/** 「检查延迟」连测几次，用来抵消网络抖动。 */
const LATENCY_ROUNDS = 3;

const KIND_LABEL: Record<AiProviderKind, string> = {
  openai_compatible: "OpenAI 兼容站点",
  deepl: "DeepL",
  ollama: "本机 Ollama",
  typesafe: "TypeSafe jev",
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

/** 粗略判断站点是不是本机服务：本机 Ollama 或回环地址都算本地。 */
function providerIsLocal(provider: AiProvider | undefined): boolean {
  if (!provider) return false;
  if (provider.kind === "ollama") return true;
  try {
    const host = new URL(provider.baseUrl).hostname.toLowerCase();
    return host === "localhost" || host === "127.0.0.1" || host === "::1";
  } catch {
    return false;
  }
}

/** AI 设置页对外接口。
 *
 * 通知智能识别开关的落盘统一交给外层设置页，避免设置页和这里各存一份互相覆盖。
 */
export interface AiPanelProps {
  /** 开关当前值；由外层设置快照决定。 */
  notifyAiEnabled: boolean;
  /** 保存开关；抛错表示失败，页面据此提示。 */
  onNotifyAiEnabledChange: (next: boolean) => Promise<void>;
}

/** 从站点地址里取主机名；解析失败时退回原串，只用于确认弹窗展示。 */
function hostOf(baseUrl: string): string {
  try {
    return new URL(baseUrl).hostname;
  } catch {
    return baseUrl;
  }
}

/** 固定色板：与后端和数据库约束保持一致；界面只允许从这里选。 */
const MAIL_LABEL_COLORS = [
  "gray",
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "indigo",
  "purple",
  "pink",
] as const;

/** 色板键在界面上的中文名。 */
const LABEL_COLOR_LABEL: Record<string, string> = {
  gray: "灰",
  red: "红",
  orange: "橙",
  amber: "琥珀",
  green: "绿",
  teal: "青",
  blue: "蓝",
  indigo: "靛",
  purple: "紫",
  pink: "粉",
};

/** AI 设置页：站点管理、功能级配置、审计与熔断。 */
export default function AiPanel({ notifyAiEnabled, onNotifyAiEnabledChange }: AiPanelProps) {
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
  // 「邮件自动分类」开关与把握程度门槛；只存本机，不参与账号同步。
  const [classifyEnabled, setClassifyEnabled] = useState(false);
  const [classifyPercent, setClassifyPercent] = useState(75);
  // 打开自动分类开关前的风险确认：先把要展示的目标记下来，点确认才落盘。
  const [classifyRisk, setClassifyRisk] = useState<
    | { providerLabel: string; host: string; model: string; local: boolean }
    | null
  >(null);
  const [classifyNotice, setClassifyNotice] = useState("");
  const [classifyError, setClassifyError] = useState("");
  // 「分类标签管理」：全部标签、新增输入与独立错误；成功操作后一律重新拉取。
  const [labels, setLabels] = useState<MailLabel[]>([]);
  const [labelBusy, setLabelBusy] = useState(false);
  const [labelError, setLabelError] = useState("");
  const [newLabelName, setNewLabelName] = useState("");
  // 打开「通知智能识别」开关前的风险确认：先记下要展示的目标，用户点确认才落盘。
  const [verifyRisk, setVerifyRisk] = useState<
    | { providerLabel: string; model: string; local: boolean }
    | null
  >(null);
  // 通知识别开关自己的保存结果；不跟站点/映射的提示串台。
  const [verifyNotice, setVerifyNotice] = useState("");
  const [verifyError, setVerifyError] = useState("");
  // 「检查延迟」：确认框、测试中标记、每次结果和错误。
  const [latencyRisk, setLatencyRisk] = useState<
    | { providerLabel: string; model: string; local: boolean }
    | null
  >(null);
  const [latencyRunning, setLatencyRunning] = useState(false);
  const [latencyResults, setLatencyResults] = useState<NotificationLatency[]>([]);
  const [latencyError, setLatencyError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [nextProviders, nextMaps, nextAudits, nextSettings, nextLabels] = await Promise.all([
        api.listAiProviders(),
        api.listAiModelMaps(),
        api.listAiAudit(50),
        api.getAppSettings(),
        api.listMailLabels(),
      ]);
      setProviders(nextProviders);
      setAudits(nextAudits);
      setLabels(nextLabels);
      setClassifyEnabled(nextSettings.classifyEnabled);
      setClassifyPercent(nextSettings.classifyConfidencePercent);
      setFeatureDrafts((old) => {
        const next: Record<string, FeatureDraftState> = {};
        for (const fn of [...FUNCTION_ORDER, VERIFY_FUNCTION]) {
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
      setError(t("请填写站点名称"));
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
      setNotice(t("已保存「{0}」", [saved.label]));
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
      setModelsHint(t("连接成功，拉到 {0} 个模型。", [models.length]));
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
        setNotice(t("已拉取 {0} 个模型", [models.length]));
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
      if (!window.confirm(t("确定删除 AI 站点「{0}」吗？相关缓存也会清理。", [provider.label]))) return;
      setBusy(true);
      setError("");
      setNotice("");
      try {
        await api.deleteAiProvider(provider.id);
        setNotice(t("已删除「{0}」", [provider.label]));
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
        setError(t("请先为「{0}」选择站点", [t(FUNCTION_LABEL[fn])]));
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
        setNotice(t("已保存「{0}」的模型设置", [t(FUNCTION_LABEL[fn])]));
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
        setNotice(t("已清除「{0}」的单独设置，将回退站点默认", [t(FUNCTION_LABEL[fn])]));
        await refresh();
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  /** 测试一个已存站点的连接；TypeSafe jev 没有模型列表接口，只做一次试探分类。 */
  const testProvider = useCallback(async (provider: AiProvider) => {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const models = await api.testAiProvider(
        provider.kind,
        provider.baseUrl,
        undefined,
        provider.id,
      );
      setNotice(
        models.length > 0
          ? t("连接成功，站点可用模型：{0}", [models.join(t("、"))])
          : t("连接成功。"),
      );
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, []);

  /** 当前「通知智能识别」选中的站点与模型；没选或站点不存在时为 undefined。 */
  const verifyDraft = featureDrafts[VERIFY_FUNCTION] ?? {
    providerId: "",
    model: "",
    thinkingLevel: "" as const,
  };
  const verifyProvider = providers.find(
    (provider) => String(provider.id) === verifyDraft.providerId,
  );

  /** 当前「邮件分类」选中的站点与模型；没选或站点不存在时为 undefined。 */
  const classifyDraft = featureDrafts.classify ?? {
    providerId: "",
    model: "",
    thinkingLevel: "" as const,
  };
  const classifyProvider = providers.find(
    (provider) => String(provider.id) === classifyDraft.providerId,
  );

  /**
   * 点「邮件自动分类」开关：开 → 先检查站点与模型，再弹一次性确认；
   * 关 → 立即落盘，后台从下一轮起停止外发。
   */
  const toggleClassify = useCallback(
    async (next: boolean) => {
      setClassifyNotice("");
      setClassifyError("");
      if (!next) {
        try {
          const saved = await api.setClassifySettings(false, classifyPercent);
          setClassifyEnabled(saved.classifyEnabled);
          setClassifyNotice(t("已关闭邮件自动分类，后续新邮件不再自动发给 AI。"));
        } catch (caught) {
          setClassifyError(describeError(caught));
        }
        return;
      }
      if (!classifyDraft.providerId || !classifyProvider) {
        setClassifyError(t("请先在「邮件分类」里选择 TypeSafe jev 站点，再打开自动分类。"));
        return;
      }
      if (!classifyProvider.enabled) {
        setClassifyError(t("所选站点已停用，请先启用或换一个站点。"));
        return;
      }
      if (classifyProvider.kind !== "typesafe") {
        setClassifyError(t("邮件分类只支持 TypeSafe jev 站点。"));
        return;
      }
      const model = classifyDraft.model.trim() || classifyProvider.defaultModel.trim();
      if (model === "") {
        setClassifyError(t("请先给邮件分类填写模型，再打开自动分类。"));
        return;
      }
      setClassifyRisk({
        providerLabel: classifyProvider.label,
        host: hostOf(classifyProvider.baseUrl),
        model,
        local: providerIsLocal(classifyProvider),
      });
    },
    [classifyDraft.model, classifyDraft.providerId, classifyPercent, classifyProvider],
  );

  /** 确认弹窗里点「确认开启」：先把分类功能存到站点上，再打开开关。 */
  const confirmEnableClassify = useCallback(async () => {
    try {
      await api.setAiFeature(
        "classify",
        Number(classifyDraft.providerId),
        classifyDraft.model,
        classifyDraft.thinkingLevel === "" ? undefined : classifyDraft.thinkingLevel,
      );
      const saved = await api.setClassifySettings(true, classifyPercent);
      setClassifyEnabled(saved.classifyEnabled);
      setClassifyRisk(null);
      setClassifyNotice(t("已开启邮件自动分类：新邮件正文会自动发给所选 AI，不再逐封确认。"));
    } catch (caught) {
      setClassifyRisk(null);
      setClassifyError(describeError(caught));
    }
  }, [classifyDraft.model, classifyDraft.providerId, classifyDraft.thinkingLevel, classifyPercent]);

  /** 门槛输入失焦时落盘；越界的整数会被拉回 50–99。 */
  const saveClassifyPercent = useCallback(async () => {
    const clamped = Math.min(99, Math.max(50, Math.round(classifyPercent)));
    setClassifyError("");
    try {
      const saved = await api.setClassifySettings(classifyEnabled, clamped);
      setClassifyPercent(saved.classifyConfidencePercent);
    } catch (caught) {
      setClassifyError(describeError(caught));
    }
  }, [classifyEnabled, classifyPercent]);

  /** 重新拉一遍标签；所有标签操作成功后都走它，不靠本地猜顺序或颜色。 */
  const reloadLabels = useCallback(async () => {
    setLabels(await api.listMailLabels());
  }, []);

  /** 新增标签；颜色由后端按固定色板自动分配。 */
  const addLabel = useCallback(async () => {
    const name = newLabelName.trim();
    if (name === "") {
      setLabelError(t("标签名不能为空"));
      return;
    }
    setLabelBusy(true);
    setLabelError("");
    try {
      await api.createMailLabel(name);
      setNewLabelName("");
      await reloadLabels();
    } catch (caught) {
      setLabelError(describeError(caught));
    } finally {
      setLabelBusy(false);
    }
  }, [newLabelName, reloadLabels]);

  /** 改名：失焦时提交；空名字直接退回重新拉取的名称。 */
  const saveLabelName = useCallback(
    async (label: MailLabel) => {
      const name = label.name.trim();
      if (name === "") {
        setLabelError(t("标签名不能为空"));
        await reloadLabels();
        return;
      }
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.renameMailLabel(label.id, name);
        await reloadLabels();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadLabels],
  );

  /** 改颜色：只接受固定色板键；兜底标签在界面上禁用。 */
  const changeLabelColor = useCallback(
    async (label: MailLabel, color: string) => {
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.setMailLabelColor(label.id, color);
        await reloadLabels();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadLabels],
  );

  /** 删除：先确认；兜底标签在界面上禁用。 */
  const removeLabel = useCallback(
    async (label: MailLabel) => {
      if (
        !window.confirm(
          t("删除「{0}」后，贴了这个标签的邮件会变成没有标签。确定删除吗？", [label.name]),
        )
      ) {
        return;
      }
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.deleteMailLabel(label.id);
        await reloadLabels();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadLabels],
  );

  /** 上下调序：把当前顺序整体提交，成功后再重新拉取。 */
  const moveLabel = useCallback(
    async (labelId: number, delta: number) => {
      const index = labels.findIndex((label) => label.id === labelId);
      const target = index + delta;
      if (index < 0 || target < 0 || target >= labels.length) return;
      const next = labels.map((label) => label.id);
      [next[index], next[target]] = [next[target], next[index]];
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.reorderMailLabels(next);
        await reloadLabels();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [labels, reloadLabels],
  );

  /**
   * 点开关时的入口：开 → 先检查站点模型，再弹风险确认；关 → 立即落盘。
   *
   * 站点或模型没配好时不放行开启，只在通知识别这一块给出中文提示，不动全局错误。
   */
  const toggleNotifyAi = useCallback(
    async (next: boolean) => {
      setVerifyNotice("");
      setVerifyError("");
      if (!next) {
        try {
          await onNotifyAiEnabledChange(false);
          setVerifyNotice(t("已关闭通知智能识别，后续新邮件不再自动发给 AI。"));
        } catch (caught) {
          setVerifyError(describeError(caught));
        }
        return;
      }
      if (!verifyDraft.providerId || !verifyProvider) {
        setVerifyError(t("请先选择站点和模型，再打开通知智能识别。"));
        return;
      }
      if (!verifyProvider.enabled) {
        setVerifyError(t("所选站点已停用，请先启用或换一个站点，再打开通知智能识别。"));
        return;
      }
      setVerifyRisk({
        providerLabel: verifyProvider.label,
        model: verifyDraft.model.trim() || verifyProvider.defaultModel,
        local: providerIsLocal(verifyProvider),
      });
    },
    [onNotifyAiEnabledChange, verifyDraft.model, verifyDraft.providerId, verifyProvider],
  );

  /**
   * 风险确认弹窗里点「确认开启」：这时才真正落盘。
   *
   * 先把弹窗里展示的站点和模型存到「通知智能识别」这个功能上，再打开开关；
   * 否则后台拿不到这个映射，会退回站点默认，用的就不是用户刚看到的目标。
   */
  const confirmEnableNotifyAi = useCallback(async () => {
    try {
      await api.setAiFeature(
        VERIFY_FUNCTION,
        Number(verifyDraft.providerId),
        verifyDraft.model,
        verifyDraft.thinkingLevel === "" ? undefined : verifyDraft.thinkingLevel,
      );
      await onNotifyAiEnabledChange(true);
      setVerifyRisk(null);
      setVerifyNotice(t("已开启通知智能识别：新邮件正文会自动发往所选 AI，不再逐封确认。"));
    } catch (caught) {
      setVerifyRisk(null);
      setVerifyError(describeError(caught));
    }
  }, [onNotifyAiEnabledChange, verifyDraft.model, verifyDraft.providerId, verifyDraft.thinkingLevel]);

  /** 点「检查延迟」：先按当前草稿做前置检查，通过才弹确认框。 */
  const startLatencyCheck = useCallback(() => {
    setLatencyError("");
    setLatencyResults([]);
    if (!verifyDraft.providerId || !verifyProvider) {
      setLatencyError(t("请先选择站点和模型，再检查延迟。"));
      return;
    }
    if (!verifyProvider.enabled) {
      setLatencyError(t("所选站点已停用，请先启用或换一个站点，再检查延迟。"));
      return;
    }
    setLatencyRisk({
      providerLabel: verifyProvider.label,
      model: verifyDraft.model.trim() || verifyProvider.defaultModel,
      local: providerIsLocal(verifyProvider),
    });
  }, [verifyDraft.model, verifyDraft.providerId, verifyProvider]);

  /**
   * 确认框里点「开始测试」：连测三次，每测完一次就把结果贴出来。
   *
   * 用界面当前草稿里的站点和模型，不要求先保存；每封信都由后端写死，不含真实内容。
   */
  const runLatencyCheck = useCallback(async () => {
    const providerId = Number(verifyDraft.providerId);
    const level = verifyDraft.thinkingLevel === "" ? undefined : verifyDraft.thinkingLevel;
    setLatencyRisk(null);
    setLatencyRunning(true);
    setLatencyError("");
    setLatencyResults([]);
    try {
      for (let round = 0; round < LATENCY_ROUNDS; round += 1) {
        const sample = await api.checkNotificationLatency(providerId, verifyDraft.model, level);
        setLatencyResults((old) => [...old, sample]);
      }
    } catch (caught) {
      setLatencyError(describeError(caught));
    } finally {
      setLatencyRunning(false);
    }
  }, [verifyDraft.model, verifyDraft.providerId, verifyDraft.thinkingLevel]);

  /** 三次结果的平均耗时；还没结果时是 0。 */
  const latencyAverage =
    latencyResults.length === 0
      ? 0
      : Math.round(
          latencyResults.reduce((sum, one) => sum + one.elapsedMs, 0) / latencyResults.length,
        );

  const disableAll = useCallback(async () => {
    if (!window.confirm(t("确定一键关闭 AI 吗？所有站点会停用，已发出的授权也会作废。"))) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const count = await api.disableAllAi();
      setNotice(t("已关闭 AI，停用 {0} 个站点", [count]));
      await refresh();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const clearCache = useCallback(async () => {
    if (!window.confirm(t("确定清空 AI 缓存吗？清空后翻译和摘要会重新请求模型。"))) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const count = await api.clearAiCache();
      setNotice(t("已清空 {0} 条 AI 缓存", [count]));
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
          <h2>{t("AI 与翻译")}</h2>
          <p className="hint">
            {t("默认关闭。Key 只进系统保险箱；非本机地址必须用 HTTPS。每次真正外发前都会弹窗确认域名、模型和是否本地。")}</p>
        </div>
        <div className="ai-panel-actions">
          <button type="button" className="danger" onClick={() => void disableAll()} disabled={busy}>
            {t("一键关闭 AI")}</button>
          <button type="button" onClick={() => void clearCache()} disabled={busy}>
            {t("清空缓存")}</button>
        </div>
      </div>

      {loading && <p className="hint" role="status">{t("正在读取 AI 设置……")}</p>}
      {error && <p className="error" role="alert">{t("操作失败：")}{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}

      <h3>{t("AI 站点")}</h3>
      <div className="ai-provider-list">
        {providers.length === 0 && !loading && (
          <p className="hint">{t("还没有站点。添加一个后，AI 功能才会出现可用状态。")}</p>
        )}
        {providers.map((provider) => (
          <article key={provider.id} className="ai-provider-card">
            <div className="ai-provider-main">
              <div className="ai-provider-title">
                <strong>{provider.label}</strong>
                <span className={provider.enabled ? "tag ai-enabled" : "tag"}>
                  {provider.enabled ? t("已启用") : t("已停用")}
                </span>
                <span className="tag">{t(KIND_LABEL[provider.kind] ?? provider.kind)}</span>
                <span className="tag">{provider.hasKey ? t("已有密钥") : t("无密钥")}</span>
              </div>
              <div className="card-sub">{t("地址：")}{provider.baseUrl}</div>
              <div className="card-sub">
                {t("默认模型：")}{provider.defaultModel || t("未填写")}{t("；思考程度：")}{t(THINKING_LABEL[provider.thinkingLevel], undefined, "ai")}
              </div>
              <div className="card-sub">
                {t("模型：")}{provider.models.length > 0 ? provider.models.join(t("、")) : t("未拉取，可手工填写")}
              </div>
            </div>
            <div className="card-actions">
              <button type="button" onClick={() => startEdit(provider)} disabled={busy}>
                {t("编辑")}</button>
              {provider.kind === "typesafe" ? (
                <button type="button" onClick={() => void testProvider(provider)} disabled={busy}>
                  {t("测试连接")}</button>
              ) : (
                <button type="button" onClick={() => void refreshModels(provider.id)} disabled={busy}>
                  {t("拉取模型")}</button>
              )}
              <button type="button" className="danger" onClick={() => void removeProvider(provider)} disabled={busy}>
                {t("删除")}</button>
            </div>
          </article>
        ))}
      </div>

      {!editing && (
        <button type="button" className="ai-add-provider" onClick={startCreate} disabled={busy}>
          {t("新增 AI 站点")}</button>
      )}

      {editing && (
        <div className="form ai-provider-form">
          <h3>{editing.id === undefined ? t("新增 AI 站点") : t("编辑 AI 站点")}</h3>
          <div className="field-row">
            <label>
              {t("站点名称")}<input
                aria-label={t("AI 站点名称")}
                value={editing.label}
                onChange={(event) =>
                  setEditing((old) => (old ? { ...old, label: event.target.value } : old))
                }
              />
            </label>
            <label>
              {t("站点类型")}<select
                aria-label={t("AI 站点类型")}
                value={editing.kind}
                onChange={(event) => {
                  const kind = event.target.value as AiProviderKind;
                  setEditing((old) => {
                    if (!old) return old;
                    // TypeSafe jev 只有分类接口：地址与模型都有官方默认值，空着就预填。
                    const preset =
                      kind === "typesafe"
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
                <option value="openai_compatible">{t("OpenAI 兼容站点")}</option>
                <option value="deepl">DeepL</option>
                <option value="ollama">{t("本机 Ollama")}</option>
                <option value="typesafe">{t("TypeSafe jev")}</option>
              </select>
            </label>
          </div>
          <label className="secret-field">
            {t("站点地址")}<input
              aria-label={t("AI 站点地址")}
              value={editing.baseUrl}
              placeholder={
                editing.kind === "typesafe"
                  ? t("填到 /v1 为止，例如 https://api.typesafe.ai/v1，不要带 /systemone")
                  : t("例如 https://api.example.com/v1，本机 Ollama 可填 http://127.0.0.1:11434")
              }
              onChange={(event) =>
                setEditing((old) => (old ? { ...old, baseUrl: event.target.value } : old))
              }
            />
          </label>
          <label className="secret-field">
            CDKey / API Key
            <span className="secret-input-row">
              <input
                className="secret-input"
                type={showApiKey ? "text" : "password"}
                aria-label="CDKey / API Key"
                autoComplete="off"
                value={apiKey}
                placeholder={editing.id === undefined ? t("新建时填写") : t("留空沿用已存密钥，输入后替换")}
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
                  setEditing((old) => (old ? { ...old, defaultModel: event.target.value } : old))
                }
              />
            </label>
            <label>
              {t("默认思考程度")}<select
                aria-label={t("默认思考程度")}
                value={editing.thinkingLevel}
                onChange={(event) =>
                  setEditing((old) =>
                    old ? { ...old, thinkingLevel: event.target.value as AiThinkingLevel } : old,
                  )
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
          <label className="secret-field">
            {t("模型列表（每行一个，也可手工填写）")}<textarea
              aria-label={t("模型列表")}
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
            {t("启用这个站点")}</label>
          {modelsHint && <p className="hint" role="status">{modelsHint}</p>}
          <div className="form-actions">
            <button type="button" className="primary" onClick={() => void saveProvider()} disabled={busy}>
              {t("保存")}</button>
            <button type="button" onClick={() => void testDraft()} disabled={busy}>
              {editing.kind === "typesafe" ? t("测试连接") : t("测试连接并拉取模型")}</button>
            <button type="button" onClick={() => setEditing(undefined)} disabled={busy}>
              {t("取消")}</button>
          </div>
        </div>
      )}

      <h3>{t("功能级模型与思考程度")}</h3>
      <p className="hint">{t("没有单独指定时回退到站点默认模型；思考程度不支持时后端会自动降级并在结果里标注。")}</p>
      <div className="ai-feature-list">
        {FUNCTION_ORDER.map((fn) => {
          const draft = featureDrafts[fn] ?? { providerId: "", model: "", thinkingLevel: "" };
          // 只列支持这个功能的站点：分类这一行就只会出现 TypeSafe jev。
          const candidates = providers.filter((provider) =>
            provider.supportedFunctions.includes(fn),
          );
          const selected = candidates.find((provider) => String(provider.id) === draft.providerId);
          return (
            <section key={fn} className="ai-feature-card">
              <h4>{t(FUNCTION_LABEL[fn])}</h4>
              <label className="secret-field">
                {t("站点")}<select
                  aria-label={t("{0}站点", [t(FUNCTION_LABEL[fn])])}
                  value={draft.providerId}
                  onChange={(event) =>
                    setFeatureDrafts((old) => ({
                      ...old,
                      [fn]: { ...draft, providerId: event.target.value, model: "" },
                    }))
                  }
                >
                  <option value="">
                    {fn === "classify" ? t("请选择站点") : t("自动选择第一个启用站点")}
                  </option>
                  {candidates.map((provider) => (
                    <option key={provider.id} value={provider.id}>
                      {provider.label}{provider.enabled ? "" : t("（已停用）")}
                    </option>
                  ))}
                </select>
              </label>
              {candidates.length === 0 && (
                <p className="hint">
                  {fn === "classify"
                    ? t("还没有 TypeSafe jev 站点：请先到上面的「AI 站点」里新增一个，再回来选。")
                    : t("还没有支持这个功能的站点。")}
                </p>
              )}
              <label className="secret-field">
                {t("模型")}<input
                  aria-label={t("{0}模型", [t(FUNCTION_LABEL[fn])])}
                  list={`ai-models-${fn}`}
                  value={draft.model}
                  placeholder={selected?.defaultModel || t("留空回退站点默认模型")}
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
                {t("思考程度")}<select
                  aria-label={t("{0}思考程度", [t(FUNCTION_LABEL[fn])])}
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
                  <option value="">{t("回退站点默认")}</option>
                  {THINKING_ORDER.map((level) => (
                    <option key={level} value={level}>
                      {t(THINKING_LABEL[level], undefined, "ai")}
                    </option>
                  ))}
                </select>
              </label>
              <div className="form-actions">
                <button type="button" onClick={() => void saveFeature(fn)} disabled={busy}>
                  {t("保存设置")}</button>
                <button type="button" onClick={() => void clearFeature(fn)} disabled={busy}>
                  {t("恢复默认")}</button>
              </div>
            </section>
          );
        })}
      </div>

      <h3>{t("邮件自动分类")}</h3>
      <p className="hint">
        {t("打开后，新邮件正文会自动发给所选 TypeSafe jev 站点做分类；模型只返回标签名和把握程度，标签与颜色只存本机。默认关闭。")}
      </p>
      <section className="ai-feature-card">
        <label className="checkbox">
          <input
            type="checkbox"
            aria-label={t("开启邮件自动分类")}
            checked={classifyEnabled}
            onChange={(event) => void toggleClassify(event.target.checked)}
          />
          <span>{t("开启邮件自动分类")}</span>
        </label>
        <label className="secret-field">
          {t("把握程度门槛")}<input
            type="number"
            min={50}
            max={99}
            aria-label={t("把握程度门槛")}
            value={classifyPercent}
            onChange={(event) => setClassifyPercent(Number(event.target.value))}
            onBlur={() => void saveClassifyPercent()}
          />
        </label>
        <p className="hint">{t("允许 50–99，默认 75；低于门槛的邮件归到「其他」。")}</p>
        {classifyNotice && <p className="notice" role="status">{classifyNotice}</p>}
        {classifyError && (
          <p className="error" role="alert">
            {t("操作失败：")}{classifyError}
          </p>
        )}
      </section>

      <h3>{t("通知智能识别（高级功能）")}</h3>
      <p className="hint">
        {t("开启后，新邮件正文会自动发往所选 AI，用来识别验证码和验证链接；识别到就在系统通知里给出复制或打开按钮，不再逐封确认。默认关闭。")}</p>
      <section className="ai-feature-card">
        <label className="checkbox">
          <input
            type="checkbox"
            aria-label={t("开启通知智能识别")}
            checked={notifyAiEnabled}
            onChange={(event) => void toggleNotifyAi(event.target.checked)}
          />
          <span>{t("开启通知智能识别")}</span>
        </label>
        <label className="secret-field">
          {t("站点")}<select
            aria-label={t("通知识别站点")}
            value={verifyDraft.providerId}
            onChange={(event) =>
              setFeatureDrafts((old) => ({
                ...old,
                [VERIFY_FUNCTION]: {
                  ...verifyDraft,
                  providerId: event.target.value,
                  model: "",
                },
              }))
            }
          >
            <option value="">{t("请选择站点")}</option>
            {providers.map((provider) => (
              <option key={provider.id} value={provider.id}>
                {provider.label}{provider.enabled ? "" : t("（已停用）")}
              </option>
            ))}
          </select>
        </label>
        <label className="secret-field">
          {t("模型")}<input
            aria-label={t("通知识别模型")}
            list={`ai-models-${VERIFY_FUNCTION}`}
            value={verifyDraft.model}
            placeholder={verifyProvider?.defaultModel || t("留空回退站点默认模型")}
            onChange={(event) =>
              setFeatureDrafts((old) => ({
                ...old,
                [VERIFY_FUNCTION]: { ...verifyDraft, model: event.target.value },
              }))
            }
          />
        </label>
        <datalist id={`ai-models-${VERIFY_FUNCTION}`}>
          {(verifyProvider?.models ?? []).map((model) => (
            <option key={model} value={model} />
          ))}
        </datalist>
        <label className="secret-field">
          {t("思考程度")}<select
            aria-label={t("通知识别思考程度")}
            value={verifyDraft.thinkingLevel}
            onChange={(event) =>
              setFeatureDrafts((old) => ({
                ...old,
                [VERIFY_FUNCTION]: {
                  ...verifyDraft,
                  thinkingLevel: event.target.value as "" | AiThinkingLevel,
                },
              }))
            }
          >
            <option value="">{t("回退站点默认")}</option>
            {THINKING_ORDER.map((level) => (
              <option key={level} value={level}>
                {t(THINKING_LABEL[level], undefined, "ai")}
              </option>
            ))}
          </select>
        </label>
        <div className="form-actions">
          <button type="button" onClick={() => void saveFeature(VERIFY_FUNCTION)} disabled={busy}>
            {t("保存设置")}</button>
          <button
            type="button"
            onClick={startLatencyCheck}
            disabled={busy || latencyRunning}
            aria-label={t("检查延迟")}
          >
            {latencyRunning ? t("测试中…") : t("检查延迟")}</button>
        </div>
        <p className="hint">
          {t("开关默认关闭；打开时会要求确认发往哪个站点、用哪个模型、是不是本地。")}</p>
        {latencyError && (
          <p className="error" role="alert">
            {t("检查延迟失败：")}{latencyError}
          </p>
        )}
        {latencyResults.length > 0 && (
          <div className="latency-report" role="status">
            {latencyResults.map((sample, index) => (
              <p key={`round-${index + 1}`}>
                {t("第 {0} 次：", [index + 1])}
                {t("{0} 毫秒", [sample.elapsedMs])}
                {" · "}
                {sample.foundCode ? t("认出验证码") : t("没认出验证码")}
                {" · "}
                {sample.foundLink ? t("认出验证链接") : t("没认出验证链接")}
              </p>
            ))}
            {latencyRunning ? (
              <p>{t("继续测试中…")}</p>
            ) : (
              <p>{t("平均耗时：{0} 毫秒", [latencyAverage])}</p>
            )}
          </div>
        )}
        {verifyNotice && <p className="notice" role="status">{verifyNotice}</p>}
        {verifyError && (
          <p className="error" role="alert">
            {t("操作失败：")}{verifyError}
          </p>
        )}
      </section>
      <h3>{t("分类标签管理")}</h3>
      <p className="hint">
        {t("标签和颜色只存在本机，不发给模型、不参与账号同步；颜色只能从固定色板里选。")}
      </p>
      <section className="ai-feature-card ai-label-manager">
        <div className="ai-label-add">
          <label className="secret-field">
            {t("新增标签")}<input
              aria-label={t("新增标签")}
              value={newLabelName}
              maxLength={64}
              onChange={(event) => setNewLabelName(event.target.value)}
            />
          </label>
          <button type="button" onClick={() => void addLabel()} disabled={labelBusy}>
            {t("新增")}</button>
        </div>
        <ul className="ai-label-list">
          {labels.map((label, index) => (
            <li key={label.id} className="ai-label-row">
              <span
                className="inbox-row-label"
                data-label-preview={label.color}
                data-label-color={label.color}
              >
                {label.name}
              </span>
              <input
                className="ai-label-name"
                aria-label={t("{0}标签名", [label.name])}
                value={label.name}
                maxLength={64}
                onChange={(event) =>
                  setLabels((old) =>
                    old.map((item) =>
                      item.id === label.id ? { ...item, name: event.target.value } : item,
                    ),
                  )
                }
                onBlur={() => void saveLabelName(label)}
              />
              <select
                aria-label={t("{0}颜色", [label.name])}
                value={label.color}
                disabled={label.isFallback || labelBusy}
                onChange={(event) => void changeLabelColor(label, event.target.value)}
              >
                {MAIL_LABEL_COLORS.map((color) => (
                  <option key={color} value={color}>
                    {t(LABEL_COLOR_LABEL[color] ?? color)}
                  </option>
                ))}
              </select>
              <button
                type="button"
                aria-label={t("上移{0}", [label.name])}
                disabled={labelBusy || index === 0}
                onClick={() => void moveLabel(label.id, -1)}
              >
                {t("上移")}</button>
              <button
                type="button"
                aria-label={t("下移{0}", [label.name])}
                disabled={labelBusy || index === labels.length - 1}
                onClick={() => void moveLabel(label.id, 1)}
              >
                {t("下移")}</button>
              <button
                type="button"
                className="danger"
                aria-label={t("删除{0}", [label.name])}
                disabled={label.isFallback || labelBusy}
                onClick={() => void removeLabel(label)}
              >
                {t("删除")}</button>
            </li>
          ))}
        </ul>
        <p className="hint">{t("「其他」是兜底标签：不能删除，颜色固定灰色，可以改名。")}</p>
        {labelError && (
          <p className="error" role="alert">
            {t("操作失败：")}{labelError}
          </p>
        )}
      </section>

      <h3>{t("AI 调用审计")}</h3>
      <p className="hint">{t("只记录时间、功能、站点、模型、是否外发和结果，不记录邮件正文或密钥。")}</p>
      {audits.length === 0 ? (
        <p className="hint">{t("还没有 AI 调用记录。")}</p>
      ) : (
        <div className="ai-audit-wrap">
          <table className="ai-audit">
            <thead>
              <tr>
                <th>{t("时间")}</th>
                <th>{t("功能")}</th>
                <th>{t("站点")}</th>
                <th>{t("模型")}</th>
                <th>{t("目标域名")}</th>
                <th>{t("是否本地")}</th>
                <th>{t("结果")}</th>
              </tr>
            </thead>
            <tbody>
              {audits.map((audit) => (
                <tr key={audit.id}>
                  <td>{audit.createdAt}</td>
                  <td>{t(FUNCTION_LABEL[audit.function as AiFunction] ?? audit.function)}</td>
                  <td>{audit.providerLabel}</td>
                  <td>{audit.model}</td>
                  <td>{audit.targetHost}</td>
                  <td>{audit.local ? t("本地") : t("外发")}</td>
                  <td>{audit.outcome}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {classifyRisk && (
        <div className="ai-modal-backdrop" role="presentation">
          <section
            className="ai-modal"
            role="dialog"
            aria-modal="true"
            aria-label={t("确认开启邮件自动分类")}
          >
            <h3>{t("确认开启邮件自动分类")}</h3>
            <p>{t("开启后，每一封新落地邮件的正文都会自动发往下面的目标做分类，不再逐封确认。")}</p>
            <dl className="ai-modal-targets">
              <dt>{t("站点")}</dt>
              <dd>{classifyRisk.providerLabel}</dd>
              <dt>{t("完整域名")}</dt>
              <dd>{classifyRisk.host}</dd>
              <dt>{t("用哪个模型")}</dt>
              <dd>{classifyRisk.model}</dd>
              <dt>{t("是不是本地服务")}</dt>
              <dd>{classifyRisk.local ? t("是，本地服务，内容不离开这台电脑") : t("不是，内容会发送到远程站点")}</dd>
            </dl>
            <p className="hint">{t("关掉这个开关会立刻停止自动外发；已经贴好的本地标签会保留。")}</p>
            <div className="form-actions">
              <button type="button" className="primary" onClick={() => void confirmEnableClassify()}>
                {t("确认开启")}</button>
              <button type="button" onClick={() => setClassifyRisk(null)}>
                {t("取消")}</button>
            </div>
          </section>
        </div>
      )}

      {verifyRisk && (
        <div className="ai-modal-backdrop" role="presentation">
          <section
            className="ai-modal"
            role="dialog"
            aria-modal="true"
            aria-label={t("确认开启通知智能识别")}
          >
            <h3>{t("确认开启通知智能识别")}</h3>
            <p>{t("开启后，每一封新邮件正文都会自动发往下面的目标识别验证码和链接，不再逐封确认。")}</p>
            <dl className="ai-modal-targets">
              <dt>{t("站点")}</dt>
              <dd>{verifyRisk.providerLabel}</dd>
              <dt>{t("用哪个模型")}</dt>
              <dd>{verifyRisk.model}</dd>
              <dt>{t("是不是本地服务")}</dt>
              <dd>{verifyRisk.local ? t("是，本地服务，内容不离开这台电脑") : t("不是，内容会发送到远程站点")}</dd>
            </dl>
            <p className="hint">{t("关掉这个开关会立刻停止自动外发；已经弹出的通知按钮不受影响。")}</p>
            <div className="form-actions">
              <button type="button" className="primary" onClick={() => void confirmEnableNotifyAi()}>
                {t("确认开启")}</button>
              <button type="button" onClick={() => setVerifyRisk(null)}>
                {t("取消")}</button>
            </div>
          </section>
        </div>
      )}

      {latencyRisk && (
        <div className="ai-modal-backdrop" role="presentation">
          <section
            className="ai-modal"
            role="dialog"
            aria-modal="true"
            aria-label={t("确认检查延迟")}
          >
            <h3>{t("确认检查延迟")}</h3>
            <p>{t("会发送一封系统编造的测试邮件，不包含你的任何真实邮件内容，连测三次。")}</p>
            <dl className="ai-modal-targets">
              <dt>{t("站点")}</dt>
              <dd>{latencyRisk.providerLabel}</dd>
              <dt>{t("用哪个模型")}</dt>
              <dd>{latencyRisk.model}</dd>
              <dt>{t("是不是本地服务")}</dt>
              <dd>{latencyRisk.local ? t("是，本地服务，内容不离开这台电脑") : t("不是，内容会发送到远程站点")}</dd>
            </dl>
            <div className="form-actions">
              <button type="button" className="primary" onClick={() => void runLatencyCheck()}>
                {t("开始测试")}</button>
              <button type="button" onClick={() => setLatencyRisk(null)}>
                {t("取消")}</button>
            </div>
          </section>
        </div>
      )}
    </section>
  );
}