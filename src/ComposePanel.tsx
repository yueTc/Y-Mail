//! 写信 / 回复 / 转发窗格（Wave 5）。
//!
//! 设计要点：
//! - 草稿、待发、已发都走本地发件队列，界面只读本地状态；
//! - 发送必须用户点按钮，正文内容不会触发任何外发动作；
//! - 临时失败会自动重试，最多三次尝试；用尽后标失败并保留草稿，按钮可手动重试；
//! - 联系人补全、签名、附件都只在本窗格里操作，不碰凭据。

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

/** 从路径里取文件名；取不到就用原串。 */
export function basename(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, "");
  if (trimmed === "") return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
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
  const [bodyText, setBodyText] = useState("");
  const [attachments, setAttachments] = useState<ComposeAttachment[]>([]);
  const [attachmentPath, setAttachmentPath] = useState("");
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

  const sendGuard = useRef(false);

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

  /** 初始化：新建给空模板，回复 / 转发由外壳组装预填内容。 */
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const boot = async () => {
      try {
        let seed: ComposeDraft | undefined;
        if (request.kind !== "new" && request.sourceMessageId) {
          seed = await api.composeDraft(request.kind, request.sourceMessageId);
        }
        if (cancelled) return;
        const targetAccount = seed?.accountId ?? accounts[0]?.accountId;
        if (seed) {
          setToText(formatRecipients(seed.to));
          setCcText(formatRecipients(seed.cc));
          setBccText(formatRecipients(seed.bcc));
          setSubject(seed.subject);
          setBodyText(seed.bodyText || "");
          setAttachments(seed.attachments);
        }
        setAccountId(targetAccount);
        setError("");
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
        .searchContacts(accountId, token, 6)
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
    const composed =
      signatureHtml.trim() === ""
        ? textToHtml(bodyText)
        : `${textToHtml(bodyText)}<br><br>${signatureHtml}`;
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
      attachments,
    };
  }, [
    accountId,
    outboxId,
    request.kind,
    toText,
    ccText,
    bccText,
    subject,
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
          if (action === "polish") {
            setBodyText(result.text);
          } else {
            setBodyText(result.text);
          }
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
    [aiBusy, aiInstruction, bodyText],
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
      setBodyText(result.text);
      setAiDowngraded(result.thinkingDowngraded);
      setAiAuth(undefined);
      setAiAction(undefined);
    } catch (caught) {
      setAiError(describeError(caught));
    } finally {
      setAiBusy(false);
    }
  }, [aiAction, aiAuth, aiInstruction, bodyText]);

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

  /** 添加附件：按路径取文件名。 */
  const addAttachment = useCallback(() => {
    const path = attachmentPath.trim();
    if (path === "") return;
    const filename = basename(path);
    setAttachments((old) => [...old, { path, filename }]);
    setAttachmentPath("");
  }, [attachmentPath]);

  const chooseSuggestion = useCallback(
    (contact: Contact) => {
      setToText((old) => applyContact(old, contact));
      setSuggestions([]);
    },
    [],
  );

  return (
    <section className="compose-pane" aria-label="写信窗格">
      <header className="compose-head">
        <h3>{title}</h3>
        <button type="button" onClick={onClose} disabled={busy}>
          关闭
        </button>
      </header>

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

        <label className="compose-field compose-body">
          <span>正文</span>
          <textarea
            aria-label="正文"
            rows={10}
            value={bodyText}
            onChange={(event) => setBodyText(event.target.value)}
          />
        </label>

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

        <div className="compose-attachments">
          <label className="compose-field">
            <span>附件路径</span>
            <input
              aria-label="附件路径"
              value={attachmentPath}
              placeholder="粘贴本地文件完整路径"
              onChange={(event) => setAttachmentPath(event.target.value)}
            />
          </label>
          <button type="button" onClick={addAttachment}>
            添加附件
          </button>
          {attachments.length > 0 && (
            <ul className="compose-attachment-list">
              {attachments.map((item, index) => (
                <li key={`${item.path}-${index}`}>
                  <span>{item.filename || item.path}</span>
                  <button
                    type="button"
                    aria-label={`移除附件 ${item.filename}`}
                    onClick={() => setAttachments((old) => old.filter((_, i) => i !== index))}
                  >
                    移除
                  </button>
                </li>
              ))}
            </ul>
          )}
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