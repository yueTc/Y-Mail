//! 统一收件箱面板（Wave 3 起，Wave 5 加搜索与写信入口）。
//!
//! 三栏骨架：左栏账号 / 文件夹，中栏列表（含搜索结果），右栏读信或写信窗格。
//! 数据都来自外壳的只读命令；这里不接触凭据，正文交给独立的读信组件渲染。
//! 正文与搜索片段一律当普通文本处理，绝不注入 HTML。

import { listen } from "@tauri-apps/api/event";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import {
  api,
  describeError,
  type AccountInboxSummary,
  type ComposeParticipant,
  type InboxFolder,
  type InboxMessage,
  type InboxQuery,
  type InboxSummary,
  type InboxThread,
  type SearchHit,
  type SnippetSegment,
  type SyncStatus,
} from "./api";
import ComposePanel, { type ComposeRequest } from "./ComposePanel";
import MessageReader from "./MessageReader";
import FlagButton from "./FlagButton";
import AddAccountDialog from "./AddAccountDialog";
import PaneResizer from "./PaneResizer";
import {
  COMPOSE_MIN_WIDTH,
  READER_MIN_WIDTH,
  RESIZER_WIDTH,
  usePaneWidths,
} from "./usePaneWidths";

/** 每页条数；外壳上限是 500。 */
const PAGE_SIZE = 200;

/** 搜索框下方的语法提示。 */
export const SEARCH_SYNTAX_HINT =
  "支持 from: 发件人、has:attachment、is:unread、before:2026-01-01";

/** 列表里的一行：线程、平铺邮件、展开出来的子邮件，或一条搜索命中。 */
export type InboxRow =
  | { kind: "thread"; key: string; thread: InboxThread }
  | { kind: "message"; key: string; message: InboxMessage }
  | { kind: "thread-message"; key: string; message: InboxMessage }
  | { kind: "search"; key: string; hit: SearchHit };

/**
 * 列表行的像素高度。
 *
 * 行高必须和行里真正渲染的文字对齐：虚拟列表给行盒的高度小于内容时，
 * 多出来的那行会溢到下一行去（会话聚合下「来自 xxx」就是这么溢出来的）。
 * 实测：两行（发件人 + 主题）约 72.7 像素；会话子邮件多一行「来自 xxx」约
 * 91.9 像素；搜索结果多一行摘要约 95.1 像素。取整后往上留不到 1 像素的余量。
 */
export function inboxRowHeight(kind: InboxRow["kind"]): number {
  if (kind === "thread-message") return 92;
  if (kind === "search") return 96;
  return 73;
}

/** 文件夹归类的中文名。 */
const FOLDER_KIND_LABEL: Record<string, string> = {
  inbox: "收件箱",
  sent: "已发送",
  draft: "草稿箱",
  trash: "已删除",
  junk: "垃圾邮件",
  custom: "自定义",
};

/**
 * 文件夹显示名：服务器给的展示名优先，只有 INBOX 这种纯英文占位名才回退到中文归类名。
 * 这样「广告邮件」不会被分类名「垃圾邮件」顶掉，左侧也不会出现两个同名文件夹。
 */
export function folderLabel(folder: InboxFolder): string {
  const path = folder.fullPath.trim();
  if (path && path.toUpperCase() !== "INBOX") return path;
  return FOLDER_KIND_LABEL[folder.kind] ?? path;
}

/**
 * 左侧一条文件夹项：真文件夹，或者「红旗邮件」这条虚拟入口。
 *
 * 「红旗邮件」不落在服务器上，是按账号把所有标红的邮件收在一起看，
 * 所以它只在界面上存在，不占 folder 表。
 */
export type SidebarEntry =
  | { kind: "folder"; folder: InboxFolder }
  | { kind: "flagged"; accountId: number };

/**
 * 把一个账号的文件夹排成左侧顺序：文件夹照原样，
 * 只在「收件箱」后面插一条「红旗邮件」；账号没有收件箱时补在最后。
 */
export function accountSidebarEntries(
  folders: readonly InboxFolder[],
  accountId: number,
): SidebarEntry[] {
  const entries: SidebarEntry[] = [];
  let inserted = false;
  for (const folder of folders) {
    if (folder.accountId !== accountId) continue;
    entries.push({ kind: "folder", folder });
    if (folder.kind === "inbox") {
      entries.push({ kind: "flagged", accountId });
      inserted = true;
    }
  }
  if (!inserted) entries.push({ kind: "flagged", accountId });
  return entries;
}

/**
 * 顶部统一区的入口：收件箱 / 未读 / 红旗 / 草稿 / 已发送。
 *
 * 参考图里的「所有最近查看」按用户要求不做，所以这里没有那一项。
 */
export type UnifiedView = "inbox" | "unread" | "flagged" | "draft" | "sent";

/** 顶部统一区的入口顺序，照着参考图来。 */
export const UNIFIED_VIEWS: ReadonlyArray<{ id: UnifiedView; label: string }> = [
  { id: "inbox", label: "统一收件箱" },
  { id: "unread", label: "所有未读" },
  { id: "flagged", label: "所有红旗" },
  { id: "draft", label: "所有草稿" },
  { id: "sent", label: "所有已发送" },
];

/** 某一类文件夹（草稿、已发送）在各账号下的邮件合计。 */
export function folderKindMessageCount(
  folders: readonly InboxFolder[],
  kind: string,
): number {
  return folders
    .filter((folder) => folder.kind === kind)
    .reduce((sum, folder) => sum + folder.messageCount, 0);
}

/**
 * 列表空着时该说什么。
 *
 * 已经配了账号还说「先去配置账号」很误导，所以这里只在一个账号都没有时才提配置。
 */
export function inboxEmptyHint(options: {
  searchMode: boolean;
  flaggedView: boolean;
  hasAccounts: boolean;
}): string {
  if (options.searchMode) return "没有找到匹配的邮件。";
  if (options.flaggedView) return "还没有标红的邮件。在列表里点星星就能标红。";
  if (!options.hasAccounts) return "这里还没有邮件。先在「账号与代理」里配置账号并同步。";
  return "这里还没有邮件。";
}

/** 同步状态里挑“最需要注意”的一个，用于统一收件箱的聚合徽标。 */
export function worstSyncStatus(list: SyncStatus[]): SyncStatus | undefined {
  if (list.length === 0) return undefined;
  const rank = (status: SyncStatus): number => {
    if (status.needsReauth || status.state === "error") return 0;
    if (status.state === "syncing" || status.state === "backfilling" || status.state === "connecting") {
      return 1;
    }
    if (status.state === "idle" || status.state === "idle_waiting") return 2;
    return 3;
  };
  let worst = list[0];
  for (const status of list) {
    if (rank(status) < rank(worst)) worst = status;
  }
  return worst;
}

/** 同步徽标的人话说明；鼠标悬停或键盘聚焦时可见。 */
export function syncBadgeTitle(status: SyncStatus): string {
  const parts = [`账号 ${status.email}`, `状态 ${status.stateLabel}`];
  if (status.total > 0) parts.push(`进度 ${status.progress}/${status.total}`);
  else if (status.progress > 0) parts.push(`已处理 ${status.progress} 封`);
  if (status.message) parts.push(status.message);
  return parts.join("；");
}

/** 同步徽标的状态配色类名。 */
export function syncBadgeClass(status: SyncStatus): string {
  if (status.needsReauth || status.state === "error") return "sync-badge danger";
  if (status.state === "syncing" || status.state === "backfilling" || status.state === "connecting") {
    return "sync-badge syncing";
  }
  if (status.state === "stopped" || status.state === "idle_waiting") return "sync-badge paused";
  return "sync-badge";
}

/** 徽标上的短文字：状态名带进度（有进度才显示）。 */
export function syncBadgeText(status: SyncStatus): string {
  if (status.total > 0) return `${status.stateLabel} ${status.progress}/${status.total}`;
  return status.stateLabel;
}

/**
 * 判断这次同步状态是否该刷新列表：新账号第一次出现（可能已经同步完），
 * 或账号从连接 / 拉取态进入等待、出错、停止等稳定态。
 */
export function syncJustSettled(
  previous: ReadonlyMap<number, SyncStatus["state"]>,
  list: SyncStatus[],
): boolean {
  const working = (state: SyncStatus["state"]): boolean =>
    state === "connecting" || state === "syncing" || state === "backfilling";
  return list.some((status) => {
    const before = previous.get(status.accountId);
    if (working(status.state)) return false;
    return before === undefined || working(before);
  });
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

/** 把一批搜索命中铺平成虚拟滚动的行。 */
export function searchRows(hits: SearchHit[]): InboxRow[] {
  return hits.map((hit) => ({ kind: "search" as const, key: `s-${hit.message.id}`, hit }));
}

/**
 * 邮件行的键盘操作：回车 / 空格打开当前行，上下方向键在可见行之间搬焦点。
 * 虚拟列表只渲染可视行，所以方向键先在当前屏能到的行内移动。
 */
function handleRowKeyDown(
  event: ReactKeyboardEvent<HTMLElement>,
  onActivate: () => void,
): void {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    onActivate();
    return;
  }
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  const current = event.currentTarget;
  const scope = current.closest(".inbox-scroll") ?? document;
  const rows = Array.from(scope.querySelectorAll<HTMLElement>(".inbox-row[tabindex]"));
  const index = rows.indexOf(current);
  if (index < 0) return;
  const next = event.key === "ArrowDown" ? rows[index + 1] : rows[index - 1];
  next?.focus();
}
/** 列表行的发件人文本。 */
function senderText(message: InboxMessage): string {
  return message.fromName.trim() !== "" ? message.fromName : message.fromAddr;
}

/** 把搜索片段渲染成纯文本；命中处用 mark 标出来，其余原样，绝不注入 HTML。 */
export function SnippetText({ segments }: { segments: SnippetSegment[] }) {
  return (
    <span className="inbox-snippet">
      {segments.map((segment, index) =>
        segment.highlighted ? (
          <mark key={index}>{segment.text}</mark>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </span>
  );
}

/** 一行邮件（平铺行、子邮件行共用）。 */
function MessageRow({
  message,
  child,
  selected,
  onOpen,
  onToggleFlag,
}: {
  message: InboxMessage;
  child?: boolean;
  selected?: boolean;
  onOpen?: () => void;
  onToggleFlag?: (message: InboxMessage) => void;
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
      tabIndex={onOpen === undefined ? undefined : 0}
      onKeyDown={
        onOpen === undefined ? undefined : (event) => handleRowKeyDown(event, onOpen)
      }
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
            {onToggleFlag && (
              <FlagButton flagged={message.isFlagged} onToggle={() => onToggleFlag(message)} />
            )}
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

/** 一条搜索结果行：邮件摘要加高亮片段。 */
function SearchResultRow({
  hit,
  selected,
  onOpen,
  onToggleFlag,
}: {
  hit: SearchHit;
  selected?: boolean;
  onOpen: () => void;
  onToggleFlag?: (message: InboxMessage) => void;
}) {
  const message = hit.message;
  const className = ["inbox-row", selected ? "inbox-row-selected" : ""].filter(Boolean).join(" ");
  return (
    <div
      className={className}
      data-read={message.isRead ? "true" : "false"}
      onClick={onOpen}
      role="button"
      tabIndex={0}
      onKeyDown={(event) => handleRowKeyDown(event, onOpen)}
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
            {onToggleFlag && (
              <FlagButton flagged={message.isFlagged} onToggle={() => onToggleFlag(message)} />
            )}
            <span className="inbox-account-chip">{message.accountName || message.accountEmail}</span>
          </span>
        </div>
        {hit.snippet.length > 0 && (
          <div className="inbox-row-snippet">
            <SnippetText segments={hit.snippet} />
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
  onToggleFlag,
}: {
  thread: InboxThread;
  expanded: boolean;
  onToggle: () => void;
  onToggleFlag?: (message: InboxMessage) => void;
}) {
  const latest = thread.latest;
  return (
    <div
      className="inbox-row inbox-row-thread"
      role="button"
      aria-expanded={expanded}
      onClick={onToggle}
      tabIndex={0}
      onKeyDown={(event) => handleRowKeyDown(event, onToggle)}
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
            {onToggleFlag && (
              <FlagButton flagged={latest.isFlagged} onToggle={() => onToggleFlag(latest)} />
            )}
          </span>
        </div>
      </div>
      <span className="thread-arrow">{expanded ? "▾" : "▸"}</span>
    </div>
  );
}

/** 从通讯录等别处发起的一次写信请求：这里只需要预填收件人。 */
export interface InboxComposeSeed {
  /** 要填进「收件人」的人。 */
  to: ComposeParticipant[];
}

type InboxPanelProps = {
  /** 外壳递进来的写信请求；非空时打开写信窗格并预填收件人。 */
  composeSeed?: InboxComposeSeed;
  /** 请求被消费后的回调，让外壳清空，避免重复打开。 */
  onComposeSeedConsumed?: () => void;
};

/** 统一收件箱面板。 */
export default function InboxPanel({ composeSeed, onComposeSeedConsumed }: InboxPanelProps) {
  const [summary, setSummary] = useState<InboxSummary>();
  const [folders, setFolders] = useState<InboxFolder[]>([]);
  const [selectedAccount, setSelectedAccount] = useState<number>();
  const [selectedFolder, setSelectedFolder] = useState<InboxFolder>();
  const [threadMode, setThreadMode] = useState(false);
  const [unreadOnly, setUnreadOnly] = useState(false);
  // 左侧「红旗邮件」虚拟入口是否选中；它按账号看全部标红邮件。
  const [flaggedView, setFlaggedView] = useState(false);
  // 顶部统一区选中的是哪一个入口。
  const [unifiedView, setUnifiedView] = useState<UnifiedView>("inbox");
  // 左侧展开的账号集合：点一下展开，再点一下收起，多个账号可以同时展开。
  const [expandedAccounts, setExpandedAccounts] = useState<Set<number>>(new Set());

  const [threads, setThreads] = useState<InboxThread[]>([]);
  const [messages, setMessages] = useState<InboxMessage[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();

  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [children, setChildren] = useState<Map<string, InboxMessage[]>>(new Map());
  const [selectedMessage, setSelectedMessage] = useState<InboxMessage>();
  // 一次性短提示，例如「未读筛选下已移出列表」。
  const [statusNote, setStatusNote] = useState("");

  // 搜索相关状态：输入值、已提交的查询、命中结果与深拉提示。
  const [searchInput, setSearchInput] = useState("");
  const [searchRaw, setSearchRaw] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searchTotal, setSearchTotal] = useState(0);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string>();
  const [searchNote, setSearchNote] = useState("");
  // 联网补历史必须用户显式点击，单独给一个忙碌标记。
  const [searchDeepBusy, setSearchDeepBusy] = useState(false);

  // 写信窗格的打开请求；为空表示当前在收件箱。
  const [composeRequest, setComposeRequest] = useState<ComposeRequest>();
  // 有没有启用的 AI 站点；只控制按钮是否可用，不触发任何请求。
  const [aiEnabled, setAiEnabled] = useState(false);
  // 已经消费过的通讯录写信请求，用来挡住重复触发。
  const consumedComposeSeed = useRef<InboxComposeSeed | undefined>(undefined);
  // 邮箱栏「添加邮箱」弹窗；保存成功后选中新账号。
  const [addAccountOpen, setAddAccountOpen] = useState(false);
  // 各账号同步状态快照；邮件列表工具栏用它显示当前账号状态徽标。
  const [syncStatuses, setSyncStatuses] = useState<SyncStatus[]>([]);
  // 上一轮同步状态，用来判断新账号是否刚完成首次拉取。
  const lastSyncStates = useRef<Map<number, SyncStatus["state"]>>(new Map());

  const scrollRef = useRef<HTMLDivElement>(null);
  const searchBoxRef = useRef<HTMLInputElement>(null);
  // 通讯录点了「写邮件」：开写信窗格并预填收件人，然后让外壳清掉这次请求。
  useEffect(() => {
    if (!composeSeed || consumedComposeSeed.current === composeSeed) return;
    consumedComposeSeed.current = composeSeed;
    setComposeRequest({ kind: "new", to: composeSeed.to });
    onComposeSeedConsumed?.();
  }, [composeSeed, onComposeSeedConsumed]);

  // 三栏宽度：邮箱栏与邮件列表可拖动，邮件内容栏永远保底 360 像素。
  // 写信工作区最小 480 像素，普通阅读区最小 360 像素。
  const {
    widths,
    limits: paneLimits,
    setSidebar,
    setList,
    reset: resetPaneWidths,
    persist: persistPaneWidths,
  } = usePaneWidths(composeRequest ? COMPOSE_MIN_WIDTH : READER_MIN_WIDTH);

  // 顶部统一区的入口只在没选具体账号 / 文件夹时生效。
  const inUnified = selectedAccount === undefined && selectedFolder === undefined;
  /** 红旗范围：账号里的「红旗邮件」，或顶部「所有红旗」。 */
  const effectiveFlagged = flaggedView || (inUnified && unifiedView === "flagged");
  /** 未读范围：用户自己的「只看未读」开关，或顶部「所有未读」。 */
  const effectiveUnread = unreadOnly || (inUnified && unifiedView === "unread");
  /** 文件夹类型范围：顶部「所有草稿」「所有已发送」。 */
  const effectiveFolderKind =
    inUnified && (unifiedView === "draft" || unifiedView === "sent") ? unifiedView : undefined;

  /** 红旗视图按时间平铺，不折会话；其他视图照用户自己的开关来。 */
  const listThreadMode = threadMode && !effectiveFlagged;

  const query = useMemo<InboxQuery>(() => {
    const value: InboxQuery = { unreadOnly: effectiveUnread, limit: PAGE_SIZE };
    if (selectedAccount !== undefined) value.accountId = selectedAccount;
    if (selectedFolder !== undefined) value.folderId = selectedFolder.folderId;
    if (effectiveFlagged) value.flaggedOnly = true;
    if (effectiveFolderKind !== undefined) value.folderKind = effectiveFolderKind;
    return value;
  }, [selectedAccount, selectedFolder, effectiveUnread, effectiveFlagged, effectiveFolderKind]);

  /** 静默检查 AI 站点状态；默认关闭时按钮显示「需启用」。 */
  useEffect(() => {
    let cancelled = false;
    void api
      .listAiProviders()
      .then((providers) => {
        if (!cancelled) setAiEnabled(providers.some((provider) => provider.enabled));
      })
      .catch(() => {
        if (!cancelled) setAiEnabled(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /** 文件夹模式下用 folderId 收窄；不然按各账号收件箱。 */
  const currentCount = listThreadMode ? threads.length : messages.length;

  const load = useCallback(
    async (reset: boolean) => {
      setLoading(true);
      try {
        const offset = reset ? 0 : listThreadMode ? threads.length : messages.length;
        if (listThreadMode) {
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
    [query, listThreadMode, threads.length, messages.length],
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
        if (listThreadMode) {
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
  }, [query, listThreadMode]);

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

  /**
   * 搜索：默认只查本地，绝不自动联网。
   * deep=true 时才联网补一批历史，且只能由用户点按钮触发；
   * append=true 表示在已有结果后面续一页。
   */
  const runSearch = useCallback(
    async (options?: { deep?: boolean; append?: boolean }) => {
      const raw = searchInput.trim();
      if (raw === "") {
        setSearchRaw("");
        setHits([]);
        setSearchTotal(0);
        setSearchNote("");
        setSearchError(undefined);
        return;
      }
      const deep = options?.deep === true;
      const append = options?.append === true;
      const offset = append ? hits.length : 0;
      setSearching(true);
      setSearchError(undefined);
      if (deep) setSearchDeepBusy(true);
      try {
        const page = await api.searchMessages({
          raw,
          ...(selectedAccount === undefined ? {} : { accountId: selectedAccount }),
          ...(offset > 0 ? { offset } : {}),
          limit: PAGE_SIZE,
          deep,
        });
        setSearchRaw(raw);
        setHits((old) => (append ? [...old, ...page.items] : page.items));
        setSearchTotal(page.total);
        setSearchNote(
          page.deepSynced
            ? "已联网补拉一批历史"
            : page.deepError
              ? `补拉历史失败：${page.deepError}`
              : "",
        );
      } catch (caught) {
        setSearchError(describeError(caught));
      } finally {
        setSearching(false);
        if (deep) setSearchDeepBusy(false);
      }
    },
    [searchInput, selectedAccount, hits.length],
  );

  /** Ctrl + K（macOS 为 Cmd + K）把焦点送到搜索框。 */
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        searchBoxRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  /** 退出搜索，回到普通收件箱列表。 */
  const clearSearch = useCallback(() => {
    setSearchInput("");
    setSearchRaw("");
    setHits([]);
    setSearchTotal(0);
    setSearchNote("");
    setSearchError(undefined);
  }, []);

  const searchMode = searchRaw !== "";

  const rows = useMemo(
    () =>
      searchMode
        ? searchRows(hits)
        : flattenInboxRows(threads, messages, listThreadMode, expanded, children),
    [searchMode, hits, threads, messages, listThreadMode, expanded, children],
  );

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => {
      const row = rows[index];
      return row ? inboxRowHeight(row.kind) : 73;
    },
    overscan: 8,
  });

  const virtualItems = virtualizer.getVirtualItems();

  /** 滚到接近底部就补下一页；搜索模式下结果已经取全，不再补。 */
  useEffect(() => {
    if (searchMode) return;
    const last = virtualItems[virtualItems.length - 1];
    if (!last) return;
    if (loading) return;
    if (currentCount >= total) return;
    if (last.index < rows.length - 5) return;
    void load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [virtualItems, loading, currentCount, total, rows.length, searchMode]);

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

  /** 打开邮件：本地先乐观标已读，失败回滚并提示；未读筛选下移出列表。 */
  const openMessage = useCallback(
    async (message: InboxMessage) => {
      setSelectedMessage(message.isRead ? message : { ...message, isRead: true });
      if (message.isRead) return;
      setStatusNote("");
      // 乐观更新：列表行样式和未读数先动起来，接口回来再拉权威数据。
      setMessages((old) => old.map((item) => (item.id === message.id ? { ...item, isRead: true } : item)));
      setThreads((old) =>
        old.map((item) =>
          item.accountId === message.accountId && item.threadKey === message.threadKey
            ? {
                ...item,
                unreadCount: Math.max(0, item.unreadCount - 1),
                latest:
                  item.latest.id === message.id ? { ...item.latest, isRead: true } : item.latest,
              }
            : item,
        ),
      );
      try {
        await api.setMessageRead(message.id, true);
        await refreshSidebar();
        await load(true);
        if (unreadOnly) setStatusNote("已标为已读，已移出「只看未读」列表");
      } catch (caught) {
        setSelectedMessage(message);
        setError(`标记已读失败：${describeError(caught)}`);
        await load(true);
      }
    },
    [unreadOnly, refreshSidebar, load],
  );

  /** 把某个旗标状态铺到所有共享状态源：列表、会话、子邮件、搜索结果、读信页。 */
  const applyMessageFlag = useCallback((id: number, isFlagged: boolean) => {
    const patch = (item: InboxMessage): InboxMessage =>
      item.id === id ? { ...item, isFlagged } : item;
    setMessages((old) => old.map(patch));
    setThreads((old) =>
      old.map((item) =>
        item.latest.id === id ? { ...item, latest: { ...item.latest, isFlagged } } : item,
      ),
    );
    setChildren((old) => {
      let changed = false;
      const copy = new Map(old);
      for (const [key, list] of copy) {
        if (list.some((item) => item.id === id)) {
          copy.set(key, list.map(patch));
          changed = true;
        }
      }
      return changed ? copy : old;
    });
    setHits((old) =>
      old.map((hit) =>
        hit.message.id === id ? { ...hit, message: { ...hit.message, isFlagged } } : hit,
      ),
    );
    setSelectedMessage((old) => (old && old.id === id ? { ...old, isFlagged } : old));
  }, []);

  /** 切换红旗：本地先动，服务器确认失败就提示稍后自动重试。 */
  const toggleFlag = useCallback(
    async (message: InboxMessage) => {
      const next = !message.isFlagged;
      setStatusNote("");
      applyMessageFlag(message.id, next);
      try {
        const result = await api.setMessageFlagged(message.id, next);
        if (!result.synced) {
          setStatusNote(
            next
              ? "已在本机标红，服务器同步失败，稍后会自动重试"
              : "已在本机取消标红，服务器同步失败，稍后会自动重试",
          );
        }
        // 红旗视图里取消标红，这一行就该走人，和「只看未读」一个道理。
        if (effectiveFlagged && !next) {
          setMessages((old) => old.filter((item) => item.id !== message.id));
          setTotal((old) => Math.max(0, old - 1));
          if (result.synced) setStatusNote("已取消标红，已移出「红旗邮件」");
        }
      } catch (caught) {
        // 本地都没写成功：回滚到点击前的状态，别让界面骗人。
        applyMessageFlag(message.id, message.isFlagged);
        setStatusNote(`切换红旗失败：${describeError(caught)}`);
      }
    },
    [applyMessageFlag, effectiveFlagged],
  );

  // 外壳后台发现新邮件时会推一条事件过来；收到就刷新列表与未读计数。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    const start = async () => {
      try {
        const off = await listen("inbox:new-mail", () => {
          void refreshAll();
        });
        if (cancelled) {
          off();
        } else {
          unlisten = off;
        }
      } catch {
        // 单元测试与浏览器预览里没有 Tauri 事件总线，忽略即可。
      }
    };
    void start();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refreshAll]);

  /** 每两秒读一次同步状态；读不到不影响收发，静默即可。 */
  useEffect(() => {
    let cancelled = false;
    const refresh = async () => {
      try {
        const list = await api.syncStatus();
        if (!cancelled) setSyncStatuses(list);
      } catch {
        // 同步状态读不到不影响收发，静默即可。
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  /**
   * 新账号第一次同步完成，或已有账号从连接 / 拉取态进入稳定态后，
   * 再刷新账号栏和邮件列表，避免只显示空列表。
   */
  useEffect(() => {
    if (syncJustSettled(lastSyncStates.current, syncStatuses)) {
      void refreshAll();
    }
    lastSyncStates.current = new Map(
      syncStatuses.map((status) => [status.accountId, status.state]),
    );
  }, [syncStatuses, refreshAll]);

  /** 当前账号的同步状态；统一收件箱取“最需要注意”的一个。 */
  const currentSync = useMemo(() => {
    if (syncStatuses.length === 0) return undefined;
    if (selectedAccount !== undefined) {
      return syncStatuses.find((item) => item.accountId === selectedAccount);
    }
    return worstSyncStatus(syncStatuses);
  }, [syncStatuses, selectedAccount]);

  /** 同步失败或需要重新授权时的重试入口。 */
  const retrySync = useCallback(async () => {
    try {
      await api.stopSync(selectedAccount);
      await api.startSync(selectedAccount);
      setSyncStatuses(await api.syncStatus());
    } catch (caught) {
      setError(describeError(caught));
    }
  }, [selectedAccount]);

  const accounts: AccountInboxSummary[] = summary?.accounts ?? [];

  return (
    <>
    <div
      className="inbox-layout"
      style={{
        gridTemplateColumns: `${widths.sidebar}px ${RESIZER_WIDTH}px ${widths.list}px ${RESIZER_WIDTH}px minmax(0, 1fr)`,
      }}
    >
      <aside className="inbox-sidebar">
        {UNIFIED_VIEWS.map((view) => {
          // 收件箱和未读用未读计数；草稿和已发送用本地条数合计；红旗不显示计数。
          const badge =
            view.id === "inbox" || view.id === "unread"
              ? (summary?.totalUnread ?? 0)
              : folderKindMessageCount(folders, view.id);
          return (
            <button
              key={view.id}
              type="button"
              className={
                selectedAccount === undefined && unifiedView === view.id
                  ? "sidebar-item active"
                  : "sidebar-item"
              }
              onClick={() => {
                setSelectedAccount(undefined);
                setSelectedFolder(undefined);
                setFlaggedView(false);
                setUnifiedView(view.id);
              }}
            >
              <span className="sidebar-item-title">{view.label}</span>
              {badge > 0 && <span className="unread-badge">{badge}</span>}
            </button>
          );
        })}

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
                setFlaggedView(false);
                setExpandedAccounts((current) => {
                  const next = new Set(current);
                  if (next.has(account.accountId)) {
                    next.delete(account.accountId);
                  } else {
                    next.add(account.accountId);
                  }
                  return next;
                });
              }}
              aria-expanded={expandedAccounts.has(account.accountId)}
            >
              <span className="dot" style={{ background: account.color || "#888" }} />
              <span className="sidebar-item-title">
                {account.displayName || account.email}
              </span>
              {account.unreadCount > 0 && (
                <span className="unread-badge">{account.unreadCount}</span>
              )}
            </button>

            {expandedAccounts.has(account.accountId) && (
              <div className="sidebar-folders">
                {accountSidebarEntries(folders, account.accountId).map((entry) =>
                  entry.kind === "flagged" ? (
                    <button
                      key="flagged"
                      type="button"
                      className={
                        flaggedView && selectedAccount === account.accountId
                          ? "sidebar-item sidebar-folder active"
                          : "sidebar-item sidebar-folder"
                      }
                      title="这个账号里所有标红的邮件"
                      onClick={() => {
                        setSelectedAccount(account.accountId);
                        setSelectedFolder(undefined);
                        setFlaggedView(true);
                      }}
                    >
                      <span className="sidebar-item-title">红旗邮件</span>
                    </button>
                  ) : (
                    <button
                      key={entry.folder.folderId}
                      type="button"
                      className={
                        selectedFolder?.folderId === entry.folder.folderId
                          ? "sidebar-item sidebar-folder active"
                          : "sidebar-item sidebar-folder"
                      }
                      onClick={() => {
                        setSelectedAccount(entry.folder.accountId);
                        setSelectedFolder(entry.folder);
                        setFlaggedView(false);
                      }}
                    >
                      <span className="sidebar-item-title">{folderLabel(entry.folder)}</span>
                      {entry.folder.unreadCount > 0 && (
                        <span className="unread-badge">{entry.folder.unreadCount}</span>
                      )}
                    </button>
                  ),
                )}
              </div>
            )}
          </div>
        ))}
        <button
          type="button"
          className="sidebar-item sidebar-add-account"
          onClick={() => setAddAccountOpen(true)}
        >
          <span className="sidebar-item-title">＋ 添加邮箱</span>
        </button>
      </aside>

      <PaneResizer
        label="邮箱栏宽度"
        value={widths.sidebar}
        min={paneLimits.sidebar.min}
        max={paneLimits.sidebar.max}
        onChange={setSidebar}
        onReset={resetPaneWidths}
        onCommit={persistPaneWidths}
      />

      <section className="inbox-main">
        <div className="inbox-toolbar">
          <button
            type="button"
            className="primary"
            onClick={() => setComposeRequest({ kind: "new" })}
          >
            写邮件
          </button>
          <button
            type="button"
            disabled={selectedMessage === undefined}
            onClick={() =>
              selectedMessage &&
              setComposeRequest({ kind: "reply", sourceMessageId: selectedMessage.id })
            }
          >
            回复
          </button>
          <button
            type="button"
            disabled={selectedMessage === undefined}
            onClick={() =>
              selectedMessage &&
              setComposeRequest({ kind: "forward", sourceMessageId: selectedMessage.id })
            }
          >
            转发
          </button>
          <label
            className="checkbox"
            title={effectiveFlagged ? "红旗邮件按时间平铺显示，不折会话" : undefined}
          >
            <input
              type="checkbox"
              checked={threadMode}
              disabled={effectiveFlagged}
              onChange={(event) => setThreadMode(event.target.checked)}
            />
            按会话聚合
          </label>
          <label
            className="checkbox"
            title={inUnified && unifiedView === "unread" ? "「所有未读」已经只看未读了" : undefined}
          >
            <input
              type="checkbox"
              checked={effectiveUnread}
              disabled={inUnified && unifiedView === "unread"}
              onChange={(event) => setUnreadOnly(event.target.checked)}
            />
            只看未读
          </label>
          <button type="button" onClick={() => void refreshAll()} disabled={loading}>
            刷新
          </button>
          {currentSync && (
            <span
              className={syncBadgeClass(currentSync)}
              tabIndex={0}
              role="status"
              title={syncBadgeTitle(currentSync)}
            >
              {syncBadgeText(currentSync)}
            </span>
          )}
          {currentSync && (currentSync.needsReauth || currentSync.state === "error") && (
            <button type="button" onClick={() => void retrySync()}>
              重试
            </button>
          )}
          <span className="hint">
            {searchMode
              ? `共找到 ${searchTotal} 封，当前已显示 ${hits.length} 封`
              : `共 ${total} ${listThreadMode ? "个会话" : "封邮件"}`}
            {loading || searching ? "，正在加载……" : ""}
          </span>
        </div>

        <div className="inbox-search">
          <input
            ref={searchBoxRef}
            aria-label="搜索邮件"
            value={searchInput}
            placeholder="按 Ctrl+K 聚焦；例如：发票 from:alice has:attachment"
            onChange={(event) => setSearchInput(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void runSearch();
              if (event.key === "Escape" && searchMode) clearSearch();
            }}
          />
          <button type="button" onClick={() => void runSearch()} disabled={searching}>
            {searching ? "搜索中……" : "搜索"}
          </button>
          {searchMode && (
            <>
              <button
                type="button"
                onClick={() => void runSearch({ deep: true })}
                disabled={searching}
              >
                {searchDeepBusy ? "联网补历史中……" : "继续联网补历史"}
              </button>
              <button type="button" onClick={clearSearch}>
                退出搜索
              </button>
            </>
          )}
          <span className="hint">{SEARCH_SYNTAX_HINT}</span>
        </div>

        {error && (
          <p className="error" role="alert">
            加载失败：{error}
          </p>
        )}
        {searchError && (
          <p className="error" role="alert">
            搜索失败：{searchError}
          </p>
        )}
        {searchNote && <p className="hint inbox-search-note">{searchNote}</p>}
        {statusNote && (
          <p className="hint inbox-status-note" role="status">
            {statusNote}
          </p>
        )}
        {searchMode && hits.length > 0 && hits.length < searchTotal && (
          <div className="inbox-search-more">
            <button
              type="button"
              onClick={() => void runSearch({ append: true })}
              disabled={searching}
            >
              {searching
                ? "加载中……"
                : `继续加载（已显示 ${hits.length} / ${searchTotal}）`}
            </button>
          </div>
        )}

        <div className="inbox-scroll" ref={scrollRef}>
          {rows.length === 0 && !loading && !searching && (
            <p className="hint inbox-empty">
              {inboxEmptyHint({
                searchMode,
                flaggedView: effectiveFlagged,
                hasAccounts: accounts.length > 0,
              })}
            </p>
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
                      onToggleFlag={(message) => void toggleFlag(message)}
                    />
                  ) : row.kind === "search" ? (
                    <SearchResultRow
                      hit={row.hit}
                      selected={selectedMessage?.id === row.hit.message.id}
                      onOpen={() => void openMessage(row.hit.message)}
                      onToggleFlag={(message) => void toggleFlag(message)}
                    />
                  ) : (
                    <MessageRow
                      message={row.message}
                      child={row.kind === "thread-message"}
                      selected={selectedMessage?.id === row.message.id}
                      onOpen={() => void openMessage(row.message)}
                      onToggleFlag={(message) => void toggleFlag(message)}
                    />
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </section>

      <PaneResizer
        label="邮件列表宽度"
        value={widths.list}
        min={paneLimits.list.min}
        max={paneLimits.list.max}
        onChange={setList}
        onReset={resetPaneWidths}
        onCommit={persistPaneWidths}
      />

      <aside className="inbox-reader">
        {composeRequest ? (
          <ComposePanel
            request={composeRequest}
            accounts={accounts}
            onClose={() => setComposeRequest(undefined)}
            onSent={() => void refreshAll()}
            aiEnabled={aiEnabled}
          />
        ) : (
          <MessageReader
            message={selectedMessage}
            aiEnabled={aiEnabled}
            onToggleFlag={(message) => void toggleFlag(message)}
          />
        )}
      </aside>
    </div>
    <AddAccountDialog
      open={addAccountOpen}
      source="mailbox"
      onClose={() => setAddAccountOpen(false)}
      onSaved={(accountId) => {
        setAddAccountOpen(false);
        void refreshSidebar();
        const id = Number(accountId);
        if (Number.isFinite(id)) {
          setSelectedAccount(id);
          setSelectedFolder(undefined);
          setExpandedAccounts((current) => new Set(current).add(id));
        }
      }}
    />
    </>
  );
}
