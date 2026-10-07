//! 写信 / 回复 / 转发窗格（Wave 5）。
//!
//! 设计要点：
//! - 草稿、待发、已发都走本地发件队列，界面只读本地状态；
//! - 发送必须用户点按钮，正文内容不会触发任何外发动作；
//! - 临时失败会自动重试，最多三次尝试；用尽后标失败并保留草稿，按钮可手动重试；
//! - 联系人补全、签名、附件都只在本窗格里操作，不碰凭据。

import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  api,
  describeError,
  type AccountInboxSummary,
  type AiAuthorization,
  type ComposeAttachment,
  type ComposeDraft,
  type ComposeParticipant,
  type Contact,
  type OutboxDraft,
  type OutboxItem,
  type OutboxKind,
  type Signature,
} from "./api";
import AiAuthorizationDialog from "./AiAuthorizationDialog";
import {
  mergeAttachments,
  pathsToAttachments,
  pointInsideRect,
  toCssPoint,
} from "./composeAttachments";
import {
  COMPOSE_DRAFT_STORAGE_KEY,
  deserializeComposeDraft,
  hasUnsavedChanges,
  serializeComposeDraft,
  type ComposeDraftAttachment,
  type ComposeDraftSnapshot,
} from "./composeDraft";
import { htmlForEditor, htmlForSending, type ResolvedInlineImage } from "./composeRichText";
import { subscribeFileDrop } from "./fileDrop";
import RichTextEditor from "./RichTextEditor";


/** 一次发送最多调用几轮发送命令：1 次首发 + 2 次重试。 */
export const MAX_SEND_ROUNDS = 3;

/** 发件状态的展示文案。 */
const STATE_LABEL: Record<string, string> = {
  draft: "草稿",
  queued: "待发送",
  sending: "发送中",
  sent: "已发送",
  failed: "发送失败",
};

/** 写信窗格要打开的请求：新建，或对某封邮件回复 / 转发。 */
export interface ComposeRequest {
  kind: OutboxKind;
  /** 回复 / 转发时的原邮件编号；新建时省略。 */
  sourceMessageId?: number;
  /** 新建时要预填进「收件人」的人；从通讯录点「写邮件」进来时用。 */
  to?: ComposeParticipant[];
}

/** 把一段纯文本转成安全的 HTML 正文（换行转 br，特殊字符转义）。 */
export function textToHtml(text: string): string {
  const escaped = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
  return escaped.replace(/\r?\n/g, "<br>");
}


/**
 * 解析收件人输入框：支持 `名字 <a@b>` 与裸地址 `a@b`，用逗号 / 分号 / 换行分隔。
 * 只保留地址非空的项。
 */
export function parseRecipients(text: string): ComposeParticipant[] {
  return text
    .split(/[,;\n]/)
    .map((piece) => piece.trim())
    .filter((piece) => piece !== "")
    .map((piece) => {
      const match = piece.match(/^(.*?)<([^>]+)>$/);
      if (match) {
        return { name: match[1].trim(), address: match[2].trim() };
      }
      return { name: "", address: piece };
    })
    .filter((person) => person.address !== "");
}

/** 把收件人列表拼回输入框文本。 */
export function formatRecipients(people: ComposeParticipant[]): string {
  return people
    .map((person) => (person.name.trim() === "" ? person.address : `${person.name} <${person.address}>`))
    .join(", ");
}

/**
 * 把一批联系人并进收件人输入框；已经在里面的地址不重复加。
 * 通讯录点「写邮件」时用它预填，不覆盖用户已经敲进去的人。
 */
export function mergeRecipients(
  text: string,
  people: readonly ComposeParticipant[],
): string {
  const merged = parseRecipients(text);
  const seen = new Set(merged.map((person) => person.address.toLowerCase()));
  for (const person of people) {
    const address = person.address.trim();
    if (address === "" || seen.has(address.toLowerCase())) continue;
    seen.add(address.toLowerCase());
    merged.push({ name: person.name.trim(), address });
  }
  return formatRecipients(merged);
}

/** 取输入框里最后一个待补全的词（按逗号 / 分号分隔）。 */
export function lastRecipientToken(text: string): string {
  const pieces = text.split(/[,;]/);
  return (pieces[pieces.length - 1] ?? "").trim();
}

/** 把最后一个待补全的词替换成选中的联系人。 */
export function applyContact(text: string, contact: Contact): string {
  const separator = text.lastIndexOf(",");
  const head = separator >= 0 ? `${text.slice(0, separator + 1)} ` : "";
  const label =
    contact.name.trim() === "" ? contact.email : `${contact.name} <${contact.email}>`;
  return `${head}${label}`;
}

/**
 * 把草稿里的内嵌图片重新读成 data URL，恢复编辑器里的显示。
 * 图片文件不在了就跳过：让它显示成裂图，不拦着用户继续写信。
 */
async function resolveInlineImages(
  attachments: readonly ComposeDraftAttachment[],
): Promise<ResolvedInlineImage[]> {
  const resolved: ResolvedInlineImage[] = [];
  for (const item of attachments) {
    const contentId = item.contentId?.trim();
    if (!contentId || item.path.trim() === "") continue;
    try {
      const info = await api.readInlineImage(item.path);
      resolved.push({
        contentId,
        path: item.path,
        filename: item.filename || info.filename,
        dataUrl: info.dataUrl,
      });
    } catch {
      // 读不出来就先不管，正文里那张图会显示成裂图。
    }
  }
  return resolved;
}

interface ComposePanelProps {
  /** 打开请求：新建 / 回复 / 转发。 */
  request: ComposeRequest;
  /** 可选的发信账号列表。 */
  accounts: AccountInboxSummary[];
  /** 是否禁用关闭（发送中）。 */
  onClose: () => void;
  /** 发送成功后的回调，用来刷新收件箱。 */
  onSent?: () => void;
  /** 当前有没有启用的 AI 站点；没有时按钮显示「需启用」。 */
  aiEnabled?: boolean;
}

/** 写信窗格。 */
export default function ComposePanel({ request, accounts, onClose, onSent, aiEnabled = false }: ComposePanelProps) {
  const [accountId, setAccountId] = useState<number>();
  const [toText, setToText] = useState("");
  const [ccText, setCcText] = useState("");
  const [bccText, setBccText] = useState("");
  const [subject, setSubject] = useState("");
  const [bodyHtml, setBodyHtml] = useState("");
  const [bodyText, setBodyText] = useState("");
  const [attachments, setAttachments] = useState<ComposeAttachment[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [signature, setSignature] = useState<Signature>();
  const [signatureOn, setSignatureOn] = useState(false);
  const [signatureDraft, setSignatureDraft] = useState("");
  const [outboxId, setOutboxId] = useState<number>();
  const [suggestions, setSuggestions] = useState<Contact[]>([]);
  const [outbox, setOutbox] = useState<OutboxItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [statusText, setStatusText] = useState("");
  const [error, setError] = useState("");
  const [aiAction, setAiAction] = useState<"polish" | "draft">();
  const [aiInstruction, setAiInstruction] = useState("");
  const [aiBusy, setAiBusy] = useState(false);
  const [aiError, setAiError] = useState("");
  const [aiDowngraded, setAiDowngraded] = useState(false);
  const [aiAuth, setAiAuth] = useState<AiAuthorization>();

  const [restoredFromBackup, setRestoredFromBackup] = useState(false);
  const [baselineReady, setBaselineReady] = useState(false);
  const [closePromptOpen, setClosePromptOpen] = useState(false);

  const sendGuard = useRef(false);
  const baselineRef = useRef<ComposeDraftSnapshot | undefined>(undefined);
  const attachmentsRef = useRef<ComposeAttachment[]>([]);
  const paneRef = useRef<HTMLElement | null>(null);

  const title = request.kind === "reply" ? "回复" : request.kind === "forward" ? "转发" : "写邮件";

  /** 读取某个账号的发件箱最近记录。 */
  const refreshOutbox = useCallback(
    async (target?: number) => {
      const id = target ?? accountId;
      if (!id) return;
      try {
        setOutbox(await api.listOutbox(id, 20));
      } catch {
        // 发件箱列表读不到不影响撰写，静默即可。
      }
    },
    [accountId],
  );

  /** 生成当前编辑内容的纯值快照，用来判断有没有未保存改动。 */
  const captureSnapshot = useCallback(
    (): ComposeDraftSnapshot => {
      // 本地备份里不存 base64：正文里的图片先换成 cid 引用，
      // 图片本身跟着附件清单一起存（只存路径），恢复时再读回来。
      const { html, images } = htmlForSending(bodyHtml);
      return {
        accountId,
        toText,
        ccText,
        bccText,
        subject,
        bodyHtml: html,
        bodyText,
        attachments: [
          ...attachmentsRef.current.map((item) => ({ path: item.path, filename: item.filename })),
          ...images.map((item) => ({
            path: item.path,
            filename: item.filename,
            contentId: item.contentId,
          })),
        ],
        signatureOn,
      };
    },
    [accountId, toText, ccText, bccText, subject, bodyHtml, bodyText, signatureOn, attachments],
  );

  /** 关闭前清掉本地备份，避免下次打开又冒出来。 */
  const clearBackup = useCallback(() => {
    try {
      window.localStorage.removeItem(COMPOSE_DRAFT_STORAGE_KEY);
    } catch {
      // 本地存储不可用时忽略，不能因此阻断关闭。
    }
  }, []);

  /** 初始化：新建时优先恢复本地备份，回复 / 转发由外壳组装预填内容。 */
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setBaselineReady(false);
    const boot = async () => {
      try {
        let seed: ComposeDraft | undefined;
        if (request.kind !== "new" && request.sourceMessageId) {
          seed = await api.composeDraft(request.kind, request.sourceMessageId);
        }
        if (cancelled) return;
        const targetAccount = seed?.accountId ?? accounts[0]?.accountId;
        let restored = false;
        if (!seed && request.kind === "new") {
          let raw: string | null = null;
          try {
            raw = window.localStorage.getItem(COMPOSE_DRAFT_STORAGE_KEY);
          } catch {
            raw = null;
          }
          const backup = deserializeComposeDraft(raw);
          if (backup && backup.kind === "new") {
            const restoredInline = await resolveInlineImages(backup.attachments);
            setToText(backup.toText);
            setCcText(backup.ccText);
            setBccText(backup.bccText);
            setSubject(backup.subject);
            setBodyText(backup.bodyText);
            setBodyHtml(
              htmlForEditor(backup.bodyHtml || textToHtml(backup.bodyText), restoredInline),
            );
            setAttachments(
              backup.attachments
                .filter((item) => !item.contentId)
                .map((item) => ({ path: item.path, filename: item.filename })),
            );
            setSignatureOn(backup.signatureOn);
            if (backup.accountId !== undefined) {
              setAccountId(backup.accountId);
            } else {
              setAccountId(targetAccount);
            }
            setRestoredFromBackup(true);
            restored = true;
          }
        }
        if (!restored && seed) {
          setToText(formatRecipients(seed.to));
          setCcText(formatRecipients(seed.cc));
          setBccText(formatRecipients(seed.bcc));
          setSubject(seed.subject);
          setBodyText(seed.bodyText || "");
          setBodyHtml(
            seed.bodyHtml.trim() === "" ? textToHtml(seed.bodyText || "") : seed.bodyHtml,
          );
          setAttachments(seed.attachments);
        }
        if (!restored) {
          setAccountId(targetAccount);
        }
        // 通讯录点进来的预填收件人：拼在恢复出来的内容后面，不重复。
        const preset = request.to ?? [];
        if (preset.length > 0) {
          setToText((current) => mergeRecipients(current, preset));
        }
        setError("");
        setBaselineReady(true);
      } catch (caught) {
        if (!cancelled) setError(describeError(caught));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void boot();
    return () => {
      cancelled = true;
    };
    // 只在打开时初始化一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 附件列表随时同步到 ref，供快照和自动草稿读取最新值。 */
  useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);

  /**
   * 初始化完成后，只记一次“刚打开时”的内容，作为未保存判断的基准。
   * 基准一旦定下就不再随编辑变化，否则永远判断成“没改动”。
   */
  useEffect(() => {
    if (!baselineReady || baselineRef.current !== undefined) return;
    baselineRef.current = captureSnapshot();
    // captureSnapshot 用 ref 读取附件，这里只在初始化时取一次，故意不放进依赖。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [baselineReady]);

  /** 停止编辑约 1 秒后，把草稿写进本地备份；内容没变就不重复写。 */
  useEffect(() => {
    if (!baselineReady || !baselineRef.current) return;
    const current = captureSnapshot();
    if (!hasUnsavedChanges(current, baselineRef.current)) {
      try {
        window.localStorage.removeItem(COMPOSE_DRAFT_STORAGE_KEY);
      } catch {
        // 本地存储不可用时忽略。
      }
      return;
    }
    const handle = setTimeout(() => {
      try {
        window.localStorage.setItem(
          COMPOSE_DRAFT_STORAGE_KEY,
          serializeComposeDraft(current, request.kind, new Date().toISOString()),
        );
        setRestoredFromBackup(true);
      } catch {
        // 本地存储写不进去时只放弃这次备份，不影响继续编辑。
      }
    }, 1000);
    return () => clearTimeout(handle);
  }, [baselineReady, captureSnapshot, request.kind]);

  /** Esc 键：关确认框，或走带保护的关闭流程。 */
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (busy) return;
      event.preventDefault();
      if (closePromptOpen) {
        setClosePromptOpen(false);
        return;
      }
      if (restoredFromBackup || hasUnsavedChanges(captureSnapshot(), baselineRef.current)) {
        setClosePromptOpen(true);
        return;
      }
      clearBackup();
      onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [busy, closePromptOpen, restoredFromBackup, captureSnapshot, clearBackup, onClose]);

  /**
   * 关闭应用 / 刷新前再拦一道，尽量别丢没保存的内容。
   * 这里只能弹浏览器原生的「确定离开吗」，三选一确认框仍由上面的关闭流程负责。
   */
  useEffect(() => {
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (busy) return;
      const dirty =
        restoredFromBackup || hasUnsavedChanges(captureSnapshot(), baselineRef.current);
      if (!dirty) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [busy, restoredFromBackup, captureSnapshot]);

  /** 账号变化时读取签名与发件箱。 */
  useEffect(() => {
    if (!accountId) return;
    let cancelled = false;
    void (async () => {
      try {
        const next = await api.getSignature(accountId);
        if (cancelled) return;
        setSignature(next);
        setSignatureDraft(next.html);
        setSignatureOn(next.enabled);
      } catch {
        if (!cancelled) setSignature(undefined);
      }
    })();
    void refreshOutbox(accountId);
    return () => {
      cancelled = true;
    };
  }, [accountId, refreshOutbox]);

  /** 收件人输入框做简单的联系人补全。 */
  useEffect(() => {
    if (!accountId) return;
    const token = lastRecipientToken(toText);
    if (token.length < 2) {
      setSuggestions([]);
      return;
    }
    let cancelled = false;
    const handle = setTimeout(() => {
      api
        .searchContacts(token, 6)
        .then((list) => {
          if (!cancelled) setSuggestions(list);
        })
        .catch(() => {
          if (!cancelled) setSuggestions([]);
        });
    }, 180);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [toText, accountId]);

  const draftPayload = useMemo<OutboxDraft | undefined>(() => {
    if (!accountId) return undefined;
    const signatureHtml = signatureOn && signature?.html.trim() ? signature.html : "";
    // 正文里的图片换成 cid: 引用，图片本身进附件清单（带编号），
    // 这样邮件不会把整张 base64 塞进正文，也不会撑爆单封上限。
    const { html, images } = htmlForSending(bodyHtml);
    const composed = signatureHtml.trim() === "" ? html : `${html}<br><br>${signatureHtml}`;
    return {
      ...(outboxId === undefined ? {} : { id: outboxId }),
      accountId,
      kind: request.kind,
      to: parseRecipients(toText),
      cc: parseRecipients(ccText),
      bcc: parseRecipients(bccText),
      subject,
      bodyHtml: composed,
      bodyText,
      inReplyTo: null,
      references: [],
      attachments: [
        ...attachments,
        ...images.map((item) => ({
          path: item.path,
          filename: item.filename,
          contentId: item.contentId,
        })),
      ],
    };
  }, [
    accountId,
    outboxId,
    request.kind,
    toText,
    ccText,
    bccText,
    subject,
    bodyHtml,
    bodyText,
    attachments,
    signature,
    signatureOn,
  ]);

  /** 保存草稿，返回发件队列编号。 */
  const saveDraft = useCallback(async (): Promise<number> => {
    if (!draftPayload) throw new Error("请先选择发信账号");
    const id = await api.saveDraft(draftPayload);
    setOutboxId(id);
    return id;
  }, [draftPayload]);

  /** 手动存草稿。 */
  const handleSaveDraft = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const id = await saveDraft();
      await refreshOutbox();
      setStatusText(`草稿已保存（编号 ${id}）`);
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [busy, saveDraft, refreshOutbox]);

  /** 跑发送队列：临时失败自动重试，次数用尽就停。 */
  const runQueue = useCallback(
    async (id: number) => {
      let sent = false;
      let lastErrors: string[] = [];
      for (let round = 0; round < MAX_SEND_ROUNDS && !sent; round += 1) {
        const outcome = await api.sendOutbox();
        lastErrors = outcome.errors;
        const item = await api.getOutbox(id);
        if (item?.state === "sent") {
          sent = true;
          break;
        }
        if (item?.state === "failed") break;
      }
      await refreshOutbox();
      if (sent) {
        // 发送成功就清掉本地兜底草稿，避免下次打开又恢复出来。
        try {
          window.localStorage.removeItem(COMPOSE_DRAFT_STORAGE_KEY);
        } catch {
          // 本地存储不可用时忽略。
        }
        setRestoredFromBackup(false);
        setStatusText("已发送");
        onSent?.();
        return;
      }
      const item = await api.getOutbox(id);
      setStatusText(
        lastErrors.length > 0
          ? lastErrors.join("；")
          : item?.lastError ?? "发送失败，草稿已保留，可稍后重试",
      );
    },
    [refreshOutbox, onSent],
  );

  /** 点发送：先存草稿再入队；重复点只跑一次。 */
  const handleSend = useCallback(async () => {
    if (sendGuard.current) return;
    sendGuard.current = true;
    setBusy(true);
    setError("");
    setStatusText("正在发送……");
    try {
      const id = await saveDraft();
      await api.enqueueOutbox(id);
      await runQueue(id);
    } catch (caught) {
      setError(describeError(caught));
      setStatusText("");
    } finally {
      sendGuard.current = false;
      setBusy(false);
    }
  }, [saveDraft, runQueue]);

  /** 手动重试一条失败件。 */
  const handleRetry = useCallback(
    async (id: number) => {
      if (sendGuard.current) return;
      sendGuard.current = true;
      setBusy(true);
      setError("");
      try {
        await api.retryOutbox(id);
        await runQueue(id);
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        sendGuard.current = false;
        setBusy(false);
      }
    },
    [runQueue],
  );

  /** AI 返回的都是纯文本：正文同时写回 HTML 与纯文本两份。 */
  const applyAiText = useCallback((text: string) => {
    setBodyText(text);
    setBodyHtml(textToHtml(text));
  }, []);

  /** 打开一个 AI 动作：先只做预览，不发送正文。 */
  const previewAi = useCallback(
    async (action: "polish" | "draft") => {
      if (aiBusy) return;
      const source = action === "polish" ? bodyText.trim() : aiInstruction.trim();
      if (source === "") {
        setAiError(action === "polish" ? "请先写正文再润色" : "请先写一句起草要求");
        return;
      }
      setAiBusy(true);
      setAiError("");
      setAiAction(action);
      try {
        const preview = await api.aiAuthorizationPreview(action, {
          text: source,
        });
        if (preview.fromCache) {
          const result =
            action === "polish"
              ? await api.polishText(source, "")
              : await api.draftText(source, "");
          applyAiText(result.text);
          setAiDowngraded(result.thinkingDowngraded);
          setAiAction(undefined);
          return;
        }
        setAiAuth(preview);
      } catch (caught) {
        setAiError(describeError(caught));
        setAiAction(undefined);
      } finally {
        setAiBusy(false);
      }
    },
    [aiBusy, aiInstruction, bodyText, applyAiText],
  );

  /** 用户确认后才真正调用模型；结果只写回纯文本正文框。 */
  const confirmAi = useCallback(async () => {
    if (!aiAuth || !aiAction) return;
    const source = aiAction === "polish" ? bodyText.trim() : aiInstruction.trim();
    setAiBusy(true);
    setAiError("");
    try {
      const result =
        aiAction === "polish"
          ? await api.polishText(source, aiAuth.authorizationToken)
          : await api.draftText(source, aiAuth.authorizationToken);
      applyAiText(result.text);
      setAiDowngraded(result.thinkingDowngraded);
      setAiAuth(undefined);
      setAiAction(undefined);
    } catch (caught) {
      setAiError(describeError(caught));
    } finally {
      setAiBusy(false);
    }
  }, [aiAction, aiAuth, aiInstruction, bodyText, applyAiText]);

  /** 保存签名。 */
  const handleSaveSignature = useCallback(async () => {
    if (!accountId) return;
    try {
      const next = await api.saveSignature(accountId, signatureDraft, signatureOn);
      setSignature(next);
      setStatusText("签名已保存");
    } catch (caught) {
      setError(describeError(caught));
    }
  }, [accountId, signatureDraft, signatureOn]);

  /** 把一批本地路径加进附件列表；重复的路径跳过，不重复挂同一个文件。 */
  const addAttachmentPaths = useCallback((paths: readonly string[]) => {
    const incoming = pathsToAttachments(paths);
    if (incoming.length === 0) return;
    const current = attachmentsRef.current;
    const merged = mergeAttachments(current, incoming);
    const added = merged.length - current.length;
    const skipped = incoming.length - added;
    if (added > 0) {
      attachmentsRef.current = merged;
      setAttachments(merged);
    }
    setStatusText(
      skipped === 0
        ? `已添加 ${added} 个附件`
        : added === 0
          ? `这 ${skipped} 个附件已经在列表里`
          : `已添加 ${added} 个附件，跳过 ${skipped} 个重复项`,
    );
  }, []);

  /** 走系统资源管理器选文件；选完把绝对路径加进附件列表。 */
  const chooseAttachments = useCallback(async () => {
    try {
      const chosen = await open({ multiple: true, title: "选择附件" });
      if (!chosen) return;
      addAttachmentPaths(Array.isArray(chosen) ? chosen : [chosen]);
    } catch (caught) {
      setError(describeError(caught));
    }
  }, [addAttachmentPaths]);


  /**
   * 拖拽文件进写信窗格：只认落点在窗格里的那些。
   * 坐标由外壳按物理像素给，这里换算成网页像素再和窗格矩形比。
   */
  useEffect(() => {
    return subscribeFileDrop((event) => {
      const pane = paneRef.current;
      const point =
        event.position && pane
          ? toCssPoint(event.position, window.devicePixelRatio)
          : undefined;
      const inside =
        point !== undefined && pane !== null
          ? pointInsideRect(point, pane.getBoundingClientRect())
          : false;
      if (event.type === "drop") {
        setDragActive(false);
        if (inside) addAttachmentPaths(event.paths);
        return;
      }
      if (event.type === "leave") {
        setDragActive(false);
        return;
      }
      setDragActive(inside);
    });
  }, [addAttachmentPaths]);

  const chooseSuggestion = useCallback(
    (contact: Contact) => {
      setToText((old) => applyContact(old, contact));
      setSuggestions([]);
    },
    [],
  );

  /** 关闭按钮：有未保存改动就先弹三选一，否则直接关。 */
  const handleCloseRequest = useCallback(() => {
    if (busy) return;
    if (restoredFromBackup || hasUnsavedChanges(captureSnapshot(), baselineRef.current)) {
      setClosePromptOpen(true);
      return;
    }
    clearBackup();
    onClose();
  }, [busy, restoredFromBackup, captureSnapshot, clearBackup, onClose]);

  /** 选择“保存草稿”：写进发件队列后再关，失败就留在窗里报错。 */
  const confirmSaveAndClose = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await saveDraft();
      clearBackup();
      setClosePromptOpen(false);
      onClose();
    } catch (caught) {
      setError(describeError(caught));
      setClosePromptOpen(false);
    } finally {
      setBusy(false);
    }
  }, [busy, saveDraft, clearBackup, onClose]);

  /** 选择“放弃修改”：清本地备份后直接关。 */
  const confirmDiscardAndClose = useCallback(() => {
    clearBackup();
    setClosePromptOpen(false);
    onClose();
  }, [clearBackup, onClose]);

  return (
    <section className="compose-pane" aria-label="写信窗格" ref={paneRef}>
      <header className="compose-head">
        <h3>
          {title}
          {restoredFromBackup && <span className="compose-draft-badge">草稿</span>}
        </h3>
        <button type="button" onClick={handleCloseRequest} disabled={busy}>
          关闭
        </button>
      </header>

      {closePromptOpen && (
        <div className="ai-modal-backdrop" role="presentation">
          <section
            className="ai-modal"
            role="dialog"
            aria-modal="true"
            aria-label="关闭写信窗格"
          >
            <h3>还有内容没保存</h3>
            <p>直接关闭会丢掉没保存的内容，要先存成草稿吗？</p>
            <div className="form-actions">
              <button
                type="button"
                className="primary"
                onClick={() => void confirmSaveAndClose()}
                disabled={busy}
              >
                保存草稿
              </button>
              <button type="button" onClick={confirmDiscardAndClose} disabled={busy}>
                放弃修改
              </button>
              <button type="button" onClick={() => setClosePromptOpen(false)} disabled={busy}>
                取消关闭
              </button>
            </div>
          </section>
        </div>
      )}

      {aiAuth && aiAction && (
        <AiAuthorizationDialog
          preview={aiAuth}
          busy={aiBusy}
          onCancel={() => {
            setAiAuth(undefined);
            setAiAction(undefined);
          }}
          onConfirm={() => void confirmAi()}
        />
      )}
      {loading && <p className="hint">正在准备内容……</p>}
      {error && <p className="error">操作失败：{error}</p>}

      <div className="compose-form">
        <label className="compose-field">
          <span>发信账号</span>
          <select
            aria-label="发信账号"
            value={accountId ?? ""}
            onChange={(event) => setAccountId(Number(event.target.value))}
          >
            <option value="">请选择账号</option>
            {accounts.map((account) => (
              <option key={account.accountId} value={account.accountId}>
                {account.displayName || account.email}
              </option>
            ))}
          </select>
        </label>

        <label className="compose-field">
          <span>收件人</span>
          <input
            aria-label="收件人"
            value={toText}
            placeholder="名字 <a@b.com>，多个用逗号分开"
            onChange={(event) => setToText(event.target.value)}
          />
        </label>
        {suggestions.length > 0 && (
          <ul className="compose-suggestions" aria-label="联系人建议">
            {suggestions.map((contact) => (
              <li key={contact.id}>
                <button type="button" onClick={() => chooseSuggestion(contact)}>
                  {contact.name.trim() || contact.email}
                  <span className="compose-suggestion-addr">{contact.email}</span>
                </button>
              </li>
            ))}
          </ul>
        )}

        <label className="compose-field">
          <span>抄送</span>
          <input aria-label="抄送" value={ccText} onChange={(event) => setCcText(event.target.value)} />
        </label>

        <label className="compose-field">
          <span>密送</span>
          <input aria-label="密送" value={bccText} onChange={(event) => setBccText(event.target.value)} />
        </label>

        <label className="compose-field">
          <span>主题</span>
          <input aria-label="主题" value={subject} onChange={(event) => setSubject(event.target.value)} />
        </label>

        <div className="compose-ai">
          <div className="compose-ai-row">
            <button
              type="button"
              disabled={!aiEnabled || aiBusy}
              onClick={() => void previewAi("polish")}
              title={aiEnabled ? "润色正文" : "需先在设置里启用 AI 站点"}
            >
              {aiBusy && aiAction === "polish" ? "润色中……" : aiEnabled ? "AI 润色" : "AI 润色（需启用）"}
            </button>
            <label>
              起草要求
              <input
                aria-label="起草要求"
                value={aiInstruction}
                placeholder="例如：写一封礼貌的项目进度询问邮件"
                onChange={(event) => setAiInstruction(event.target.value)}
                disabled={!aiEnabled}
              />
            </label>
            <button
              type="button"
              disabled={!aiEnabled || aiBusy}
              onClick={() => void previewAi("draft")}
              title={aiEnabled ? "按起草要求生成内容" : "需先在设置里启用 AI 站点"}
            >
              {aiBusy && aiAction === "draft" ? "起草中……" : aiEnabled ? "AI 起草" : "AI 起草（需启用）"}
            </button>
          </div>
          {aiDowngraded && <p className="hint">该模型不支持所选思考程度，已按默认调用。</p>}
          {aiError && <p className="error">AI 操作失败：{aiError}</p>}
        </div>

        <div className="compose-field compose-body">
          <span>正文</span>
          <RichTextEditor
            value={bodyHtml}
            onChange={(html, text) => {
              setBodyHtml(html);
              setBodyText(text);
            }}
            onAttach={() => void chooseAttachments()}
            attachments={attachments}
            onRemoveAttachment={(index) =>
              setAttachments((old) => old.filter((_, i) => i !== index))
            }
            dropActive={dragActive}
            disabled={busy}
          />
        </div>

        <div className="compose-signature">
          <label className="checkbox">
            <input
              type="checkbox"
              checked={signatureOn}
              onChange={(event) => setSignatureOn(event.target.checked)}
            />
            附加签名
          </label>
          <textarea
            aria-label="签名内容"
            rows={3}
            value={signatureDraft}
            placeholder="这个账号的签名，支持简单 HTML"
            onChange={(event) => setSignatureDraft(event.target.value)}
          />
          <button type="button" onClick={() => void handleSaveSignature()}>
            保存签名
          </button>
        </div>


      </div>

      <footer className="compose-actions">
        <button type="button" onClick={() => void handleSaveDraft()} disabled={busy}>
          存草稿
        </button>
        <button type="button" className="primary" onClick={() => void handleSend()} disabled={busy}>
          发送
        </button>
        {statusText && <span className="compose-status">{statusText}</span>}
      </footer>

      <section className="compose-outbox">
        <h4>发件箱（最近）</h4>
        {outbox.length === 0 && <p className="hint">这里暂时没有记录。</p>}
        <ul>
          {outbox.map((item) => (
            <li key={item.id} className={`compose-outbox-item state-${item.state}`}>
              <span className="compose-outbox-subject">{item.subject || "（无主题）"}</span>
              <span className="compose-outbox-state">
                {STATE_LABEL[item.state] ?? item.state}
                {item.attempts > 0 ? `（已尝试 ${item.attempts} 次）` : ""}
              </span>
              {item.lastError && <span className="compose-outbox-error">{item.lastError}</span>}
              {item.state === "failed" && (
                <button type="button" disabled={busy} onClick={() => void handleRetry(item.id)}>
                  重试
                </button>
              )}
            </li>
          ))}
        </ul>
      </section>
    </section>
  );
}