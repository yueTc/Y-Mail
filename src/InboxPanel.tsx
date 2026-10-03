//! 统一收件箱面板（Wave 3）。
//!
//! 三栏骨架：左栏账号 / 文件夹，中栏虚拟滚动列表，右栏读信窗格。
//! 数据全部来自外壳的只读命令；这里不接触凭据，正文交给独立的读信组件渲染。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import {
  api,
  describeError,
  type AccountInboxSummary,
  type InboxFolder,
  type InboxMessage,
  type InboxQuery,
  type InboxSummary,
  type InboxThread,
} from "./api";
import MessageReader from "./MessageReader";

/** 每页条数；外壳上限是 500。 */
const PAGE_SIZE = 200;

/** 列表里的一行：线程、平铺邮件或展开出来的子邮件。 */
export type InboxRow =
  | { kind: "thread"; key: string; thread: InboxThread }
  | { kind: "message"; key: string; message: InboxMessage }
  | { kind: "thread-message"; key: string; message: InboxMessage };

/** 文件夹归类的中文名。 */
const FOLDER_KIND_LABEL: Record<string, string> = {
  inbox: "收件箱",
  sent: "已发送",
  draft: "草稿箱",
  trash: "已删除",
  junk: "垃圾邮件",
  custom: "自定义",
};

/** 文件夹显示名：已知归类用中文名，自定义文件夹用服务器路径（更有辨识度）。 */
export function folderLabel(folder: InboxFolder): string {
  if (folder.kind === "custom") return folder.fullPath;
  return FOLDER_KIND_LABEL[folder.kind] ?? folder.fullPath;
}

/** 把时间整理成列表里的短文本：今天只给时分，昨天给「昨天」，更早给月日。 */
export function formatListTime(iso: string, now: Date = new Date()): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;

  const startOfDay = (value: Date) =>
    new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const dayDiff = Math.round((startOfDay(now) - startOfDay(date)) / 86400000);
  if (dayDiff <= 0) {
    const hh = String(date.getHours()).padStart(2, "0");
    const mm = String(date.getMinutes()).padStart(2, "0");
    return `${hh}:${mm}`;
  }
  if (dayDiff === 1) return "昨天";
  const mm = String(date.getMonth() + 1).padStart(2, "0");
  const dd = String(date.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}

/** 把线程 / 邮件列表铺平成虚拟滚动的行；展开的线程把子邮件插在自己后面。 */
export function flattenInboxRows(
  threads: InboxThread[],
  messages: InboxMessage[],
  threadMode: boolean,
  expanded: ReadonlySet<string>,
  children: ReadonlyMap<string, readonly InboxMessage[]>,
): InboxRow[] {
  if (!threadMode) {
    return messages.map((message) => ({
      kind: "message" as const,
      key: `m-${message.id}`,
      message,
    }));
  }
  const rows: InboxRow[] = [];
  for (const thread of threads) {
    const key = `${thread.accountId}:${thread.threadKey}`;
    rows.push({ kind: "thread" as const, key, thread });
    if (!expanded.has(key)) continue;
    for (const message of children.get(key) ?? []) {
      rows.push({
        kind: "thread-message" as const,
        key: `${key}:${message.id}`,
        message,
      });
    }
  }
  return rows;
}

/** 列表行的发件人文本。 */
function senderText(message: InboxMessage): string {
  return message.fromName.trim() !== "" ? message.fromName : message.fromAddr;
}

/** 一行邮件（平铺行、子邮件行共用）。 */
function MessageRow({
  message,
  child,
  selected,
  onOpen,
}: {
  message: InboxMessage;
  child?: boolean;
  selected?: boolean;
  onOpen?: () => void;
}) {
  const className = ["inbox-row", child ? "inbox-row-child" : "", selected ? "inbox-row-selected" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      className={className}
      data-read={message.isRead ? "true" : "false"}
      onClick={onOpen}
      role={onOpen ? "button" : undefined}
    >
      <span className="dot" style={{ background: message.accountColor || "#888" }} />
      <div className="inbox-row-main">
        <div className="inbox-row-top">
          <span className="inbox-row-sender">{senderText(message)}</span>
          <span className="inbox-row-time">{formatListTime(message.dateUtc)}</span>
        </div>
        <div className="inbox-row-bottom">
          <span className="inbox-row-subject">{message.subject || "（无主题）"}</span>
          <span className="inbox-row-marks">
            {message.hasAttachments && <span title="有附件">📎</span>}
            {message.isFlagged && <span title="已星标">★</span>}
          </span>
        </div>
        {child && (
          <div className="inbox-row-sub">
            来自 {message.accountName || message.accountEmail}
          </div>
        )}
      </div>
    </div>
  );
}

/** 一行会话线程。 */
function ThreadRow({
  thread,
  expanded,
  onToggle,
}: {
  thread: InboxThread;
  expanded: boolean;
  onToggle: () => void;
}) {
  const latest = thread.latest;
  return (
    <div
      className="inbox-row inbox-row-thread"
      role="button"
      aria-expanded={expanded}
      onClick={onToggle}
    >
      <span className="dot" style={{ background: latest.accountColor || "#888" }} />
      <div className="inbox-row-main">
        <div className="inbox-row-top">
          <span className="inbox-row-sender">{senderText(latest)}</span>
          <span className="inbox-row-time">{formatListTime(latest.dateUtc)}</span>
        </div>
        <div className="inbox-row-bottom">
          <span className="inbox-row-subject">
            {latest.subject || "（无主题）"}
            <span className="thread-count">{thread.messageCount}</span>
          </span>
          <span className="inbox-row-marks">
            {thread.unreadCount > 0 && (
              <span className="unread-badge" title={`未读 ${thread.unreadCount} 封`}>
                {thread.unreadCount}
              </span>
            )}
            {latest.hasAttachments && <span title="有附件">📎</span>}
          </span>
        </div>
      </div>
      <span className="thread-arrow">{expanded ? "▾" : "▸"}</span>
    </div>
  );
}

/** 统一收件箱面板。 */
export default function InboxPanel() {
  const [summary, setSummary] = useState<InboxSummary>();
  const [folders, setFolders] = useState<InboxFolder[]>([]);
  const [selectedAccount, setSelectedAccount] = useState<number>();
  const [selectedFolder, setSelectedFolder] = useState<InboxFolder>();
  const [threadMode, setThreadMode] = useState(true);
  const [unreadOnly, setUnreadOnly] = useState(false);

  const [threads, setThreads] = useState<InboxThread[]>([]);
  const [messages, setMessages] = useState<InboxMessage[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();

  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [children, setChildren] = useState<Map<string, InboxMessage[]>>(new Map());
  const [selectedMessage, setSelectedMessage] = useState<InboxMessage>();

  const scrollRef = useRef<HTMLDivElement>(null);

  const query = useMemo<InboxQuery>(() => {
    const value: InboxQuery = { unreadOnly, limit: PAGE_SIZE };
    if (selectedAccount !== undefined) value.accountId = selectedAccount;
    if (selectedFolder !== undefined) value.folderId = selectedFolder.folderId;
    return value;
  }, [selectedAccount, selectedFolder, unreadOnly]);

  /** 文件夹模式下用 folderId 收窄；不然按各账号收件箱。 */
  const currentCount = threadMode ? threads.length : messages.length;

  const load = useCallback(
    async (reset: boolean) => {
      setLoading(true);
      try {
        const offset = reset ? 0 : threadMode ? threads.length : messages.length;
        if (threadMode) {
          const page = await api.listInboxThreads({ ...query, offset });
          setThreads((old) => (reset ? page.items : [...old, ...page.items]));
          setTotal(page.total);
        } else {
          const page = await api.listInboxMessages({ ...query, offset });
          setMessages((old) => (reset ? page.items : [...old, ...page.items]));
          setTotal(page.total);
        }
        setError(undefined);
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setLoading(false);
      }
    },
    [query, threadMode, threads.length, messages.length],
  );

  /** 第一次进来，以及筛选条件变化时：清空并重新拉第一页。 */
  useEffect(() => {
    let cancelled = false;
    setThreads([]);
    setMessages([]);
    setTotal(0);
    setExpanded(new Set());
    setChildren(new Map());
    setLoading(true);

    const boot = async () => {
      try {
        if (threadMode) {
          const page = await api.listInboxThreads({ ...query, offset: 0 });
          if (cancelled) return;
          setThreads(page.items);
          setTotal(page.total);
        } else {
          const page = await api.listInboxMessages({ ...query, offset: 0 });
          if (cancelled) return;
          setMessages(page.items);
          setTotal(page.total);
        }
        setError(undefined);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, threadMode]);

  /** 总览与文件夹：第一次进来拉一次，之后点刷新再拉。 */
  const refreshSidebar = useCallback(async () => {
    try {
      const [nextSummary, nextFolders] = await Promise.all([
        api.inboxSummary(),
        api.listInboxFolders(),
      ]);
      setSummary(nextSummary);
      setFolders(nextFolders);
    } catch (caught) {
      setError(describeError(caught));
    }
  }, []);

  useEffect(() => {
    void refreshSidebar();
  }, [refreshSidebar]);

  const rows = useMemo(
    () => flattenInboxRows(threads, messages, threadMode, expanded, children),
    [threads, messages, threadMode, expanded, children],
  );

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => (rows[index]?.kind === "thread-message" ? 56 : 68),
    overscan: 8,
  });

  const virtualItems = virtualizer.getVirtualItems();

  /** 滚到接近底部就补下一页。 */
  useEffect(() => {
    const last = virtualItems[virtualItems.length - 1];
    if (!last) return;
    if (loading) return;
    if (currentCount >= total) return;
    if (last.index < rows.length - 5) return;
    void load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [virtualItems, loading, currentCount, total, rows.length]);

  /** 展开 / 收起一条会话；第一次展开时拉子邮件。 */
  const toggleThread = useCallback(
    async (thread: InboxThread) => {
      const key = `${thread.accountId}:${thread.threadKey}`;
      const next = new Set(expanded);
      if (next.has(key)) {
        next.delete(key);
        setExpanded(next);
        return;
      }
      next.add(key);
      setExpanded(next);
      if (children.has(key)) return;
      try {
        const list = await api.listThreadMessages(thread.accountId, thread.threadKey, 200);
        setChildren((old) => {
          const copy = new Map(old);
          copy.set(key, list);
          return copy;
        });
      } catch (caught) {
        setError(describeError(caught));
      }
    },
    [expanded, children],
  );

  const refreshAll = useCallback(async () => {
    await refreshSidebar();
    await load(true);
  }, [refreshSidebar, load]);

  const accounts: AccountInboxSummary[] = summary?.accounts ?? [];

  return (
    <div className="inbox-layout">
      <aside className="inbox-sidebar">
        <button
          type="button"
          className={selectedAccount === undefined ? "sidebar-item active" : "sidebar-item"}
          onClick={() => {
            setSelectedAccount(undefined);
            setSelectedFolder(undefined);
          }}
        >
          <span className="sidebar-item-title">统一收件箱</span>
          {(summary?.totalUnread ?? 0) > 0 && (
            <span className="unread-badge">{summary?.totalUnread}</span>
          )}
        </button>

        {accounts.map((account) => (
          <div key={account.accountId} className="sidebar-account">
            <button
              type="button"
              className={
                selectedAccount === account.accountId
                  ? "sidebar-item sidebar-account-item active"
                  : "sidebar-item sidebar-account-item"
              }
              onClick={() => {
                setSelectedAccount(account.accountId);
                setSelectedFolder(undefined);
              }}
            >
              <span className="dot" style={{ background: account.color || "#888" }} />
              <span className="sidebar-item-title">
                {account.displayName || account.email}
              </span>
              {account.unreadCount > 0 && (
                <span className="unread-badge">{account.unreadCount}</span>
              )}
            </button>

            {selectedAccount === account.accountId && (
              <div className="sidebar-folders">
                {folders
                  .filter((folder) => folder.accountId === account.accountId)
                  .map((folder) => (
                    <button
                      key={folder.folderId}
                      type="button"
                      className={
                        selectedFolder?.folderId === folder.folderId
                          ? "sidebar-item sidebar-folder active"
                          : "sidebar-item sidebar-folder"
                      }
                      onClick={() => setSelectedFolder(folder)}
                    >
                      <span className="sidebar-item-title">{folderLabel(folder)}</span>
                      {folder.unreadCount > 0 && (
                        <span className="unread-badge">{folder.unreadCount}</span>
                      )}
                    </button>
                  ))}
              </div>
            )}
          </div>
        ))}
      </aside>

      <section className="inbox-main">
        <div className="inbox-toolbar">
          <label className="checkbox">
            <input
              type="checkbox"
              checked={threadMode}
              onChange={(event) => setThreadMode(event.target.checked)}
            />
            按会话聚合
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={unreadOnly}
              onChange={(event) => setUnreadOnly(event.target.checked)}
            />
            只看未读
          </label>
          <button type="button" onClick={() => void refreshAll()} disabled={loading}>
            刷新
          </button>
          <span className="hint">
            共 {total} {threadMode ? "个会话" : "封邮件"}
            {loading ? "，正在加载……" : ""}
          </span>
        </div>

        {error && <p className="error">加载失败：{error}</p>}

        <div className="inbox-scroll" ref={scrollRef}>
          {rows.length === 0 && !loading && (
            <p className="hint inbox-empty">这里还没有邮件。先在「账号与代理」里配置账号并同步。</p>
          )}
          <div
            className="inbox-virtual"
            style={{ height: virtualizer.getTotalSize(), position: "relative" }}
          >
            {virtualItems.map((item) => {
              const row = rows[item.index];
              if (!row) return null;
              return (
                <div
                  key={row.key}
                  className="inbox-virtual-item"
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${item.start}px)`,
                    height: item.size,
                  }}
                >
                  {row.kind === "thread" ? (
                    <ThreadRow
                      thread={row.thread}
                      expanded={expanded.has(row.key)}
                      onToggle={() => void toggleThread(row.thread)}
                    />
                  ) : (
                    <MessageRow
                      message={row.message}
                      child={row.kind === "thread-message"}
                      selected={selectedMessage?.id === row.message.id}
                      onOpen={() => setSelectedMessage(row.message)}
                    />
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </section>

      <aside className="inbox-reader">
        <MessageReader message={selectedMessage} />
      </aside>
    </div>
  );
}