//! AI 站点与功能设置（Wave 7）。
//!
//! 安全约定：
//! - CDKey 只通过密码框传给后端，不写浏览器存储，也不在界面回显；
//! - 站点与模型列表只展示后端返回的非敏感字段；
//! - 模型输出一律当纯文本展示，绝不使用 dangerouslySetInnerHTML。

import { useCallback, useEffect, useState } from "react";

import { PROXY_MODE_LABELS } from "./AccountForm";
import {
  api,
  describeError,
  type AccountProxy,
  type AiAudit,
  type AiFunction,
  type AiModelMap,
  type AiProvider,
  type AiThinkingLevel,
  type LabelGroup,
  type LabelGroupKind,
  type MailLabel,
  type NotificationLatency,
  type Proxy,
} from "./api";
import AiProviderDialog, {
  isDecisionKind,
  KIND_LABEL,
  THINKING_LABEL,
  THINKING_ORDER,
} from "./AiProviderDialog";
import { t } from "./i18n";
import AiTestModelPicker from "./AiTestModelPicker";

/** 编辑站点图标：铅笔；按钮名字由 aria-label 和 title 提供。 */
function EditIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M12 20h9" />
        <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
      </g>
    </svg>
  );
}

/** 卡片上显示站点代理策略；选了具体代理时补上代理名。 */
function proxyText(proxy: AccountProxy, proxies: Proxy[]): string {
  const base = t(PROXY_MODE_LABELS[proxy.mode]);
  if (proxy.mode !== "custom" || proxy.proxyId === undefined) return base;
  const found = proxies.find((item) => item.id === proxy.proxyId);
  if (!found) return base;
  const name = found.label.trim() === "" ? `${found.host}:${found.port}` : found.label;
  return `${base}（${name}）`;
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


interface FeatureDraftState {
  providerId: string;
  model: string;
  thinkingLevel: "" | AiThinkingLevel;
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

/** 标签组类型在界面上的中文名。 */
const GROUP_KIND_LABEL: Record<LabelGroupKind, string> = {
  choice: "选择题",
  boolean: "是否题",
  score: "打分题",
};

/** 开启一个组之前的检查；返回空串表示可以开，否则返回中文原因。 */
function groupEnableBlockReason(
  group: LabelGroup,
  draft: { model: string; providerId: string },
  provider: AiProvider | undefined,
): string {
  if (!draft.providerId || !provider) {
    return t("请先在「邮件分类」里选择决策模型站点，再打开组开关。");
  }
  if (!provider.enabled) {
    return t("所选站点已停用，请先启用或换一个站点。");
  }
  if (!isDecisionKind(provider.kind)) {
    return t("邮件分类只支持决策模型站点。");
  }
  const model = draft.model.trim() || provider.defaultModel.trim();
  if (model === "") {
    return t("请先给邮件分类填写模型，再打开组开关。");
  }
  if (group.kind === "choice" && group.labels.length === 0) {
    return t("这个选择题组还没有标签，先加一个标签再打开。");
  }
  if (group.kind === "boolean" && group.labels.length !== 2) {
    return t("是否题组要有两条标签才能打开。");
  }
  if (
    group.kind === "score" &&
    (group.scoreMin === null ||
      group.scoreMax === null ||
      group.scoreMax - group.scoreMin + 1 < 2)
  ) {
    return t("这个打分题组的下限和上限不合法。");
  }
  return "";
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
  // 站点弹窗：新增时 editingProvider 为 null，编辑时指向要改的那个站点。
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<AiProvider | null>(null);
  // 站点卡片默认全部收起；按站点编号分别记展开状态，只影响界面，不写配置。
  const [expandedProviders, setExpandedProviders] = useState<Record<number, boolean>>({});
  const [testModels, setTestModels] = useState<Record<number, string>>({});
  // 卡片上「指定代理」要显示代理名，这里存一份代理清单。
  const [proxies, setProxies] = useState<Proxy[]>([]);
  const [featureDrafts, setFeatureDrafts] = useState<Record<string, FeatureDraftState>>({});
  // 「邮件自动分类」总开关与手动分类免确认；只存本机，不参与账号同步。
  const [classifyEnabled, setClassifyEnabled] = useState(false);
  const [classifySkipConfirm, setClassifySkipConfirm] = useState(false);
  // 打开自动分类开关前的风险确认：先把要展示的目标记下来，点确认才落盘。
  const [classifyRisk, setClassifyRisk] = useState<
    | { providerLabel: string; host: string; model: string; local: boolean }
    | null
  >(null);
  const [classifyNotice, setClassifyNotice] = useState("");
  const [classifyError, setClassifyError] = useState("");
  // 「标签组管理」：全部标签组、新建组输入、按组的新标签输入与独立错误；成功操作后一律重新拉取。
  const [groups, setGroups] = useState<LabelGroup[]>([]);
  const [labelBusy, setLabelBusy] = useState(false);
  const [labelError, setLabelError] = useState("");
  const [newGroupName, setNewGroupName] = useState("");
  const [newGroupKind, setNewGroupKind] = useState<LabelGroupKind>("choice");
  const [newGroupScoreMin, setNewGroupScoreMin] = useState("");
  const [newGroupScoreMax, setNewGroupScoreMax] = useState("");
  const [newLabelNames, setNewLabelNames] = useState<Record<number, string>>({});
  const [thresholdDrafts, setThresholdDrafts] = useState<Record<number, string>>({});
  // 设置页里每一组可单独展开；默认全部收起，只影响界面，不写配置。
  const [expandedGroups, setExpandedGroups] = useState<Record<number, boolean>>({});
  // 「功能级模型与思考程度」里每个功能一张卡片，可单独展开；默认收起，只影响界面，不写配置。
  const [expandedFeatures, setExpandedFeatures] = useState<Record<string, boolean>>({});
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
      const [nextProviders, nextMaps, nextAudits, nextSettings, nextLabels, nextProxies] =
        await Promise.all([
          api.listAiProviders(),
          api.listAiModelMaps(),
          api.listAiAudit(50),
          api.getAppSettings(),
          api.listLabelGroups(),
          api.listProxies(),
        ]);
      setProviders(nextProviders);
      setProxies(nextProxies);
      setAudits(nextAudits);
      setGroups(nextLabels);
      setClassifyEnabled(nextSettings.classifyEnabled);
      setClassifySkipConfirm(nextSettings.classifySkipManualConfirm);
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

  const removeProvider = useCallback(
    async (provider: AiProvider) => {
      if (!window.confirm(t("确定删除 AI 站点「{0}」吗？相关缓存也会清理。", [provider.label]))) return;
      setBusy(true);
      setError("");
      setNotice("");
      try {
        await api.deleteAiProvider(provider.id);
        setNotice(t("已删除「{0}」", [provider.label]));
        await refresh();
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
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

  /** 测试一个已存决策站点的连接；这类站点没有模型列表接口，只做一次试探分类。 */
  const testProvider = useCallback(async (provider: AiProvider, model: string) => {
    setError("");
    setNotice("");
    if (model.trim() === "") {
      setError(t("请选择或填写测试模型"));
      return;
    }
    setBusy(true);
    try {
      const models = await api.testAiProvider(
        provider.kind,
        provider.baseUrl,
        undefined,
        provider.id,
        provider.proxy,
        model.trim(),
      );
      setNotice(t("连接成功，测试模型：{0}", [models[0] ?? model.trim()]));
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
  /** 打开总开关时，确认框里要写清这次会一起开哪些组。 */
  const enabledGroupNames = groups.filter((group) => group.enabled).map((group) => group.name);

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
          const saved = await api.setClassifySettings(false);
          setClassifyEnabled(saved.classifyEnabled);
          setClassifyNotice(t("已关闭邮件自动分类，后续新邮件不再自动发给 AI。"));
        } catch (caught) {
          setClassifyError(describeError(caught));
        }
        return;
      }
      if (!classifyDraft.providerId || !classifyProvider) {
        setClassifyError(t("请先在「邮件分类」里选择决策模型站点，再打开自动分类。"));
        return;
      }
      if (!classifyProvider.enabled) {
        setClassifyError(t("所选站点已停用，请先启用或换一个站点。"));
        return;
      }
      if (!isDecisionKind(classifyProvider.kind)) {
        setClassifyError(t("邮件分类只支持决策模型站点。"));
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
    [classifyDraft.model, classifyDraft.providerId, classifyProvider],
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
      const saved = await api.setClassifySettings(true);
      setClassifyEnabled(saved.classifyEnabled);
      setClassifyRisk(null);
      setClassifyNotice(t("已开启邮件自动分类：新邮件正文会自动发给所选 AI，不再逐封确认。"));
    } catch (caught) {
      setClassifyRisk(null);
      setClassifyError(describeError(caught));
    }
  }, [classifyDraft.model, classifyDraft.providerId, classifyDraft.thinkingLevel]);

  /**
   * 点「手动分类免确认」：开 → 右键「立即分类」直接外发、不再弹每封确认；
   * 关 → 恢复逐封确认。只写本机设置，改了立刻生效。
   */
  const toggleClassifySkipConfirm = useCallback(async (next: boolean) => {
    setClassifyNotice("");
    setClassifyError("");
    try {
      const saved = await api.setClassifySkipManualConfirm(next);
      setClassifySkipConfirm(saved.classifySkipManualConfirm);
      setClassifyNotice(
        next
          ? t("已开启手动分类免确认：右键「立即分类」会直接外发，不再逐封确认。")
          : t("已关闭手动分类免确认：右键「立即分类」会重新逐封确认。"),
      );
    } catch (caught) {
      setClassifyError(describeError(caught));
    }
  }, []);

  /** 重新拉一遍标签组；所有标签组和组内标签操作成功后都走它，不靠本地猜顺序或颜色。 */
  const reloadGroups = useCallback(async () => {
    setGroups(await api.listLabelGroups());
  }, []);

  /** 新建组：打分题要填上下限；是否题建好后自动配两条标签。 */
  const addGroup = useCallback(async () => {
    const name = newGroupName.trim();
    if (name === "") {
      setLabelError(t("标签组名不能为空"));
      return;
    }
    let scoreMin: number | undefined;
    let scoreMax: number | undefined;
    if (newGroupKind === "score") {
      scoreMin = Number(newGroupScoreMin);
      scoreMax = Number(newGroupScoreMax);
      if (
        newGroupScoreMin.trim() === "" ||
        newGroupScoreMax.trim() === "" ||
        !Number.isFinite(scoreMin) ||
        !Number.isFinite(scoreMax) ||
        scoreMax - scoreMin + 1 < 2
      ) {
        setLabelError(t("打分题要填下限和上限，而且上限至少要比下限大 1。"));
        return;
      }
    }
    setLabelBusy(true);
    setLabelError("");
    try {
      const created = await api.createLabelGroup(name, newGroupKind, scoreMin, scoreMax);
      if (newGroupKind === "boolean") {
        await api.createMailLabel(created.id, t("需要回复"));
        await api.createMailLabel(created.id, t("无需回复"));
      }
      setNewGroupName("");
      setNewGroupScoreMin("");
      setNewGroupScoreMax("");
      await reloadGroups();
    } catch (caught) {
      setLabelError(describeError(caught));
    } finally {
      setLabelBusy(false);
    }
  }, [newGroupKind, newGroupName, newGroupScoreMax, newGroupScoreMin, reloadGroups]);

  /** 组改名：失焦提交；空名字退回重新拉取的名称。 */
  const saveGroupName = useCallback(
    async (group: LabelGroup) => {
      const name = group.name.trim();
      if (name === "") {
        setLabelError(t("标签组名不能为空"));
        await reloadGroups();
        return;
      }
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.renameLabelGroup(group.id, name);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 组开关：只记「这一组参不参与」；真正外发仍由总开关拦着。 */
  const toggleGroup = useCallback(
    async (group: LabelGroup, next: boolean) => {
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.setLabelGroupEnabled(group.id, next);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 单个标签组展开/收起：只影响设置界面，不改数据。 */
  const toggleGroupExpanded = useCallback((groupId: number) => {
    setExpandedGroups((old) => ({ ...old, [groupId]: !old[groupId] }));
  }, []);
  /** 单个功能卡片展开/收起：只影响设置界面，不改数据。 */
  const toggleFeatureExpanded = useCallback((fn: AiFunction) => {
    setExpandedFeatures((old) => ({ ...old, [fn]: !(old[fn] === true) }));
  }, []);

  /** 组门槛：失焦落盘，越界拉回 50–99。 */
  const saveGroupThreshold = useCallback(
    async (group: LabelGroup) => {
      const raw =
        thresholdDrafts[group.id] ?? String(Math.round(group.confidenceThreshold * 100));
      const percent = Math.min(99, Math.max(50, Math.round(Number(raw))));
      setLabelError("");
      setThresholdDrafts((old) => ({ ...old, [group.id]: String(percent) }));
      try {
        await api.setLabelGroupThreshold(group.id, percent / 100);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      }
    },
    [reloadGroups, thresholdDrafts],
  );

  /** 删除组：连带组内标签和该组结果，先确认。 */
  const removeGroup = useCallback(
    async (group: LabelGroup) => {
      if (
        !window.confirm(
          t("删除「{0}」会连带删掉组内标签和这些组的分类结果，确定吗？", [group.name]),
        )
      ) {
        return;
      }
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.deleteLabelGroup(group.id);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 组上下调序：把当前顺序整体提交，成功后再重新拉取。 */
  const moveGroup = useCallback(
    async (groupId: number, delta: number) => {
      const index = groups.findIndex((item) => item.id === groupId);
      const target = index + delta;
      if (index < 0 || target < 0 || target >= groups.length) return;
      const next = groups.map((item) => item.id);
      [next[index], next[target]] = [next[target], next[index]];
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.reorderLabelGroups(next);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [groups, reloadGroups],
  );

  /** 组内新增标签；颜色由后端按固定色板自动分配。 */
  const addLabelToGroup = useCallback(
    async (groupId: number) => {
      const name = (newLabelNames[groupId] ?? "").trim();
      if (name === "") {
        setLabelError(t("标签名不能为空"));
        return;
      }
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.createMailLabel(groupId, name);
        setNewLabelNames((old) => ({ ...old, [groupId]: "" }));
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [newLabelNames, reloadGroups],
  );

  /** 标签改名：失焦提交；空名字直接退回重新拉取的名称。 */
  const saveLabelName = useCallback(
    async (label: MailLabel) => {
      const name = label.name.trim();
      if (name === "") {
        setLabelError(t("标签名不能为空"));
        await reloadGroups();
        return;
      }
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.renameMailLabel(label.groupId, label.id, name);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 改颜色：只接受固定色板键；兜底标签在界面上禁用。 */
  const changeLabelColor = useCallback(
    async (label: MailLabel, color: string) => {
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.setMailLabelColor(label.groupId, label.id, color);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 指定或换掉兜底标签：同一组最多一个，改完重新拉取。 */
  const toggleLabelFallback = useCallback(
    async (label: MailLabel, isFallback: boolean) => {
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.setMailLabelFallback(label.groupId, label.id, isFallback);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 删除标签：先确认；兜底标签在界面上禁用。 */
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
        await api.deleteMailLabel(label.groupId, label.id);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 组内标签上下调序：把该组当前顺序整体提交，成功后再重新拉取。 */
  const moveLabel = useCallback(
    async (group: LabelGroup, labelId: number, delta: number) => {
      const index = group.labels.findIndex((item) => item.id === labelId);
      const target = index + delta;
      if (index < 0 || target < 0 || target >= group.labels.length) return;
      const next = group.labels.map((item) => item.id);
      [next[index], next[target]] = [next[target], next[index]];
      setLabelBusy(true);
      setLabelError("");
      try {
        await api.reorderMailLabels(group.id, next);
        await reloadGroups();
      } catch (caught) {
        setLabelError(describeError(caught));
      } finally {
        setLabelBusy(false);
      }
    },
    [reloadGroups],
  );

  /** 组名输入先写本地草稿，失焦再提交。 */
  const setGroupNameDraft = useCallback((groupId: number, name: string) => {
    setGroups((old) => old.map((group) => (group.id === groupId ? { ...group, name } : group)));
  }, []);

  /** 组内标签改名输入先写本地草稿，失焦再提交。 */
  const setLabelNameDraft = useCallback((groupId: number, labelId: number, name: string) => {
    setGroups((old) =>
      old.map((group) =>
        group.id === groupId
          ? {
              ...group,
              labels: group.labels.map((item) => (item.id === labelId ? { ...item, name } : item)),
            }
          : group,
      ),
    );
  }, []);

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
        {providers.map((provider) => {
          const expanded = expandedProviders[provider.id] === true;
          const detailsId = `ai-provider-details-${provider.id}`;
          const testModel =
            testModels[provider.id] ??
            (provider.defaultModel.trim() ||
              provider.models.find((model) => model.trim())?.trim() ||
              "");
          return (
            <article
              key={provider.id}
              className="ai-provider-card"
              data-expanded={expanded ? "true" : "false"}
            >
              <div className="ai-provider-head">
                <button
                  type="button"
                  className="ai-provider-toggle"
                  aria-expanded={expanded}
                  aria-controls={detailsId}
                  aria-label={
                    expanded
                      ? t("折叠{0}", [provider.label])
                      : t("展开{0}", [provider.label])
                  }
                  title={expanded ? t("折叠这一项") : t("展开这一项")}
                  onClick={() =>
                    setExpandedProviders((old) => ({
                      ...old,
                      [provider.id]: !expanded,
                    }))
                  }
                >
                  <span className="ai-provider-chevron" aria-hidden="true">
                    {expanded ? "▾" : "▸"}
                  </span>
                  <strong>{provider.label}</strong>
                </button>
                <div className="ai-provider-quick-actions">
                  <button
                    type="button"
                    className="ai-provider-edit-button"
                    aria-label={t("编辑")}
                    title={t("编辑")}
                    onClick={() => {
                      setEditingProvider(provider);
                      setDialogOpen(true);
                    }}
                    disabled={busy}
                  >
                    <EditIcon />
                  </button>
                  <button
                    type="button"
                    className="danger"
                    onClick={() => void removeProvider(provider)}
                    disabled={busy}
                  >
                    {t("删除")}</button>
                </div>
              </div>
              {expanded && (
                <div id={detailsId} className="ai-provider-details">
                  <div className="ai-provider-main">
                    <div className="ai-provider-title">
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
                    <div className="card-sub">{t("代理策略：")}{proxyText(provider.proxy, proxies)}</div>
                  </div>
                  {isDecisionKind(provider.kind) && (
                    <div className="ai-provider-test">
                      <AiTestModelPicker
                        value={testModel}
                        defaultModel={provider.defaultModel}
                        models={provider.models}
                        disabled={busy}
                        onChange={(model) => {
                          setTestModels((old) => ({ ...old, [provider.id]: model }));
                          setNotice("");
                        }}
                      />
                      <button type="button" onClick={() => void testProvider(provider, testModel)} disabled={busy}>
                        {t("测试连接")}</button>
                    </div>
                  )}
                </div>
              )}
            </article>
          );
        })}
      </div>

      <button
        type="button"
        className="ai-add-provider"
        onClick={() => {
          setEditingProvider(null);
          setDialogOpen(true);
        }}
        disabled={busy}
      >
        {t("新增 AI 站点")}</button>

      <AiProviderDialog
        open={dialogOpen}
        provider={editingProvider}
        onClose={() => setDialogOpen(false)}
        onSaved={(saved) => {
          setDialogOpen(false);
          setNotice(t("已保存「{0}」", [saved.label]));
          void refresh();
        }}
      />
      <h3>{t("功能级模型与思考程度")}</h3>
      <p className="hint">{t("没有单独指定时回退到站点默认模型；思考程度不支持时后端会自动降级并在结果里标注。")}</p>
      <div className="ai-feature-list">
        {FUNCTION_ORDER.map((fn) => {
          const draft = featureDrafts[fn] ?? { providerId: "", model: "", thinkingLevel: "" };
          // 只列支持这个功能的站点：分类这一行会列出新决策站点和旧 TypeSafe。
          const candidates = providers.filter((provider) =>
            provider.supportedFunctions.includes(fn),
          );
          const selected = candidates.find((provider) => String(provider.id) === draft.providerId);
          const expanded = expandedFeatures[fn] === true;
          const collapsed = !expanded;
          const detailsId = `ai-feature-details-${fn}`;
          return (
            <section
              key={fn}
              className="ai-feature-card"
              data-expanded={expanded ? "true" : "false"}
            >
              <div className="ai-feature-head">
                <button
                  type="button"
                  className="ai-feature-toggle"
                  aria-expanded={expanded}
                  aria-controls={detailsId}
                  aria-label={
                    collapsed
                      ? t("展开{0}", [t(FUNCTION_LABEL[fn])])
                      : t("折叠{0}", [t(FUNCTION_LABEL[fn])])
                  }
                  title={collapsed ? t("展开这一项") : t("折叠这一项")}
                  onClick={() => toggleFeatureExpanded(fn)}
                >
                  <span className="ai-feature-chevron" aria-hidden="true">
                    {collapsed ? "▸" : "▾"}
                  </span>
                  <span className="ai-feature-title">{t(FUNCTION_LABEL[fn])}</span>
                </button>
              </div>
              {!collapsed && (
                <div id={detailsId} className="ai-feature-details">
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
                        ? t("还没有可用的决策模型站点：请先到上面的「AI 站点」里新增一个，再回来选。")
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
                </div>
              )}
            </section>
          );
        })}
      </div>

      <h3>{t("通知智能识别（高级功能）")}</h3>
      <p className="hint">
        {t("开启后，新邮件正文会自动发往所选 AI，用来识别验证码和验证链接；本地没有正文时会先自动下载到本机。识别到就在系统通知里给出复制或打开按钮，不再逐封确认。默认关闭。")}</p>
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
      <h3>{t("邮件自动分类 · 标签组管理")}</h3>
      <section className="ai-feature-card ai-label-manager ai-label-manager-merged">
        <div className="ai-classify-global">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label={t("开启邮件自动分类")}
              checked={classifyEnabled}
              onChange={(event) => void toggleClassify(event.target.checked)}
            />
            <span>{t("开启邮件自动分类")}</span>
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label={t("手动分类免确认")}
              checked={classifySkipConfirm}
              onChange={(event) => void toggleClassifySkipConfirm(event.target.checked)}
            />
            <span>{t("手动分类免确认")}</span>
          </label>
          <p className="hint">
            {t("勾选手动分类免确认后，右键「立即分类」会直接外发、不再逐封确认；本地没有正文时会先自动下载到本机。随时可在这里关掉。默认关闭。")}
          </p>
          {!classifyEnabled && (
            <p className="hint">{t("总开关没开，组开关打开也不外发。")}</p>
          )}
          {classifyNotice && <p className="notice" role="status">{classifyNotice}</p>}
          {classifyError && (
            <p className="error" role="alert">
              {t("操作失败：")}{classifyError}
            </p>
          )}
        </div>
        <div className="ai-label-add">
          <label className="secret-field">
            {t("新建组名")}<input
              aria-label={t("新建组名")}
              value={newGroupName}
              maxLength={64}
              onChange={(event) => setNewGroupName(event.target.value)}
            />
          </label>
          <label className="secret-field">
            {t("新建组类型")}<select
              aria-label={t("新建组类型")}
              value={newGroupKind}
              onChange={(event) => setNewGroupKind(event.target.value as LabelGroupKind)}
            >
              <option value="choice">{t("选择题")}</option>
              <option value="boolean">{t("是否题")}</option>
              <option value="score">{t("打分题")}</option>
            </select>
          </label>
          {newGroupKind === "score" && (
            <>
              <label className="secret-field">
                {t("新建组下限")}<input
                  type="number"
                  aria-label={t("新建组下限")}
                  value={newGroupScoreMin}
                  onChange={(event) => setNewGroupScoreMin(event.target.value)}
                />
              </label>
              <label className="secret-field">
                {t("新建组上限")}<input
                  type="number"
                  aria-label={t("新建组上限")}
                  value={newGroupScoreMax}
                  onChange={(event) => setNewGroupScoreMax(event.target.value)}
                />
              </label>
            </>
          )}
          <button type="button" onClick={() => void addGroup()} disabled={labelBusy}>
            {t("新建组")}</button>
        </div>
        <ul className="ai-label-group-list">
          {groups.map((group, groupIndex) => {
            const blockReason = groupEnableBlockReason(group, classifyDraft, classifyProvider);
            const expanded = expandedGroups[group.id] === true;
            const collapsed = !expanded;
            const detailsId = `ai-label-group-details-${group.id}`;
            return (
              <li
                key={group.id}
                className="ai-label-group"
                data-expanded={expanded ? "true" : "false"}
              >
                <div className="ai-label-group-head">
                  <button
                    type="button"
                    className="ai-label-group-toggle"
                    aria-expanded={expanded}
                    aria-controls={detailsId}
                    aria-label={
                      collapsed
                        ? t("展开{0}", [group.name])
                        : t("折叠{0}", [group.name])
                    }
                    title={collapsed ? t("展开这一组") : t("折叠这一组")}
                    onClick={() => toggleGroupExpanded(group.id)}
                  >
                    <span className="ai-label-group-chevron" aria-hidden="true">
                      {collapsed ? "▸" : "▾"}
                    </span>
                    <span className="ai-label-group-name">{group.name}</span>
                  </button>
                  <span className="tag">{t(GROUP_KIND_LABEL[group.kind] ?? group.kind)}</span>
                  <div className="ai-label-group-actions">
                    <button
                      type="button"
                      aria-label={t("上移{0}", [group.name])}
                      disabled={labelBusy || groupIndex === 0}
                      onClick={() => void moveGroup(group.id, -1)}
                    >
                      {t("上移")}</button>
                    <button
                      type="button"
                      aria-label={t("下移{0}", [group.name])}
                      disabled={labelBusy || groupIndex === groups.length - 1}
                      onClick={() => void moveGroup(group.id, 1)}
                    >
                      {t("下移")}</button>
                    <button
                      type="button"
                      className="danger"
                      aria-label={t("删除{0}", [group.name])}
                      disabled={labelBusy}
                      onClick={() => void removeGroup(group)}
                    >
                      {t("删除")}</button>
                    <label className="checkbox ai-label-group-enable">
                      <input
                        type="checkbox"
                        aria-label={t("开启或关闭{0}", [group.name])}
                        title={t("开启或关闭{0}", [group.name])}
                        checked={group.enabled}
                        disabled={labelBusy}
                        onChange={(event) => void toggleGroup(group, event.target.checked)}
                      />
                    </label>
                  </div>
                </div>
                {!collapsed && (
                  <div id={detailsId} className="ai-label-group-body">
                    <label className="secret-field ai-label-group-name-field">
                      {t("组名")}<input
                        className="ai-label-name"
                        aria-label={t("{0}组名", [group.name])}
                        value={group.name}
                        maxLength={64}
                        onChange={(event) => setGroupNameDraft(group.id, event.target.value)}
                        onBlur={() => void saveGroupName(group)}
                      />
                    </label>
                    <div className="ai-label-group-params">
                      <label className="secret-field">
                        {t("门槛")}<input
                          type="number"
                          min={50}
                          max={99}
                          aria-label={t("{0}门槛", [group.name])}
                          value={thresholdDrafts[group.id] ?? String(Math.round(group.confidenceThreshold * 100))}
                          onChange={(event) =>
                            setThresholdDrafts((old) => ({ ...old, [group.id]: event.target.value }))
                          }
                          onBlur={() => void saveGroupThreshold(group)}
                        />
                      </label>
                      {group.kind === "score" && (
                        <>
                          <label className="secret-field">
                            {t("下限")}<input
                              type="number"
                              aria-label={t("{0}下限", [group.name])}
                              value={group.scoreMin ?? ""}
                              readOnly
                            />
                          </label>
                          <label className="secret-field">
                            {t("上限")}<input
                              type="number"
                              aria-label={t("{0}上限", [group.name])}
                              value={group.scoreMax ?? ""}
                              readOnly
                            />
                          </label>
                        </>
                      )}
                    </div>
                    {group.enabled && blockReason !== "" && <p className="hint">{blockReason}</p>}
                    {group.kind !== "score" && (
                      <>
                        <div className="ai-label-add">
                          <label className="secret-field">
                            {t("在{0}里新增标签", [group.name])}<input
                              aria-label={t("在{0}里新增标签", [group.name])}
                              value={newLabelNames[group.id] ?? ""}
                              maxLength={64}
                              onChange={(event) =>
                                setNewLabelNames((old) => ({ ...old, [group.id]: event.target.value }))
                              }
                            />
                          </label>
                          <button
                            type="button"
                            aria-label={t("在{0}里新增", [group.name])}
                            disabled={labelBusy}
                            onClick={() => void addLabelToGroup(group.id)}
                          >
                            {t("新增")}</button>
                        </div>
                        <ul className="ai-label-list">
                          {group.labels.map((label, labelIndex) => (
                            <li key={label.id} className="ai-label-row">
                              <span
                                className="inbox-row-label"
                                data-label-preview={label.color}
                                data-label-color={label.color}
                              >
                                {label.name}
                              </span>
                              {label.isFallback && <span className="tag">{t("兜底")}</span>}
                              <input
                                className="ai-label-name"
                                aria-label={t("{0}标签名", [label.name])}
                                value={label.name}
                                maxLength={64}
                                onChange={(event) =>
                                  setLabelNameDraft(group.id, label.id, event.target.value)
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
                              {!label.isFallback && (
                                <button
                                  type="button"
                                  aria-label={t("把{0}设为兜底", [label.name])}
                                  disabled={labelBusy}
                                  onClick={() => void toggleLabelFallback(label, true)}
                                >
                                  {t("设为兜底")}</button>
                              )}
                              <button
                                type="button"
                                aria-label={t("上移{0}", [label.name])}
                                disabled={labelBusy || labelIndex === 0}
                                onClick={() => void moveLabel(group, label.id, -1)}
                              >
                                {t("上移")}</button>
                              <button
                                type="button"
                                aria-label={t("下移{0}", [label.name])}
                                disabled={labelBusy || labelIndex === group.labels.length - 1}
                                onClick={() => void moveLabel(group, label.id, 1)}
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
                        <p className="hint">
                          {t("兜底标签不能删除、颜色固定灰色，可以改名；每组最多一个。")}
                        </p>
                      </>
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
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
            <p>{t("开启后，每一封新落地邮件的正文都会自动发往下面的目标做分类，不再逐封确认。本地没有正文时会先自动下载到本机。")}</p>
            <dl className="ai-modal-targets">
              <dt>{t("站点")}</dt>
              <dd>{classifyRisk.providerLabel}</dd>
              <dt>{t("完整域名")}</dt>
              <dd>{classifyRisk.host}</dd>
              <dt>{t("用哪个模型")}</dt>
              <dd>{classifyRisk.model}</dd>
              <dt>{t("是不是本地服务")}</dt>
              <dd>{classifyRisk.local ? t("是，本地服务，内容不离开这台电脑") : t("不是，内容会发送到远程站点")}</dd>
              <dt>{t("将开启哪些组")}</dt>
              <dd>{enabledGroupNames.length > 0 ? enabledGroupNames.join(t("、")) : t("（当前没有开启的组）")}</dd>
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
            <p>{t("开启后，每一封新邮件正文都会自动发往下面的目标识别验证码和链接，不再逐封确认。本地没有正文时会先自动下载到本机。")}</p>
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