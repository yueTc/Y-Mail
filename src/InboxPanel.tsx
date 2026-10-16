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
  type MouseEvent as ReactMouseEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import {
  api,
  describeError,
  type Account,
  type AccountInboxSummary,
  type ClassifyPreview,
  type ComposeParticipant,
  type GroupResult,
  type InboxFolder,
  type InboxMessage,
  type InboxQuery,
  type InboxSummary,
  type InboxThread,
  type LabelGroup,
  type SearchHit,
  type SnippetSegment,
  type SyncStatus,
} from "./api";
import ComposePanel, { type ComposeRequest } from "./ComposePanel";
import MessageReader from "./MessageReader";
import { GroupPills } from "./labelPill";
import MessageClassificationMenu from "./MessageClassificationMenu";
import FlagButton from "./FlagButton";
import AccountDialog from "./AccountDialog";
import PaneResizer from "./PaneResizer";
import {
  COMPOSE_MIN_WIDTH,
  READER_MIN_WIDTH,
  RESIZER_WIDTH,
  usePaneWidths,
} from "./usePaneWidths";
import { t } from "./i18n";

/** 每页条数；外壳上限是 500。 */
const PAGE_SIZE = 200;

/** 搜索框的简短提示；完整命令仍可输入 --help 查看。 */
export const SEARCH_PLACEHOLDER = "输入 --help 查看搜索语法";


/** 列表里的一行：线程、平铺邮件、展开出来的子邮件，或一条搜索命中。 */
export type InboxRow =
  | { kind: "thread"; key: string; thread: InboxThread }
  | { kind: "message"; key: string; message: InboxMessage }
  | { kind: "thread-message"; key: string; message: InboxMessage }
  | { kind: "search"; key: string; hit: SearchHit };

/**
 * 列表行的初始估值和最小高度，保留现有密度。
 * 渲染后以实际内容高度更新位置，字号、字体和摘要变化时继续测量。
 */
export function inboxRowHeight(kind: InboxRow["kind"]): number {
  if (kind === "thread") return 40;
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
  const kindLabel = FOLDER_KIND_LABEL[folder.kind];
  return kindLabel === undefined ? path : t(kindLabel);
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
  { id: "inbox", label: "收件箱" },
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
  if (options.searchMode) return t("没有找到匹配的邮件。");
  if (options.flaggedView) return t("还没有标红的邮件。在列表里点星星就能标红。");
  if (!options.hasAccounts) return t("这里还没有邮件。先在「账号与代理」里配置账号并同步。");
  return t("这里还没有邮件。");
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
  const parts = [t("账号 {0}", [status.email]), t("状态 {0}", [status.stateLabel])];
  if (status.total > 0) parts.push(t("进度 {0}/{1}", [status.progress, status.total]));
  else if (status.progress > 0) parts.push(t("已处理 {0} 封", [status.progress]));
  if (status.message) parts.push(status.message);
  return parts.join(t("；"));
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

/** 徽标状态仍取最需注意的账号，数量按当前范围合计；总数未齐时不显示局部合计。 */
export function syncBadgeText(
  status: SyncStatus,
  statuses: readonly SyncStatus[] = [status],
): string {
  if (statuses.length === 0 || statuses.some((item) => item.total <= 0)) return status.stateLabel;
  const progress = statuses.reduce((sum, item) => sum + item.progress, 0);
  const total = statuses.reduce((sum, item) => sum + item.total, 0);
  return `${status.stateLabel} ${progress}/${total}`;
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
  if (dayDiff === 1) return t("昨天");
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
  onLabelClick,
}: {
  message: InboxMessage;
  child?: boolean;
  selected?: boolean;
  onOpen?: () => void;
  onToggleFlag?: (message: InboxMessage) => void;
  onLabelClick?: (result: GroupResult) => void;
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
          <GroupPills results={message.groupResults} onClick={onLabelClick} />
          <span className="inbox-row-time">{formatListTime(message.dateUtc)}</span>
        </div>
        <div className="inbox-row-bottom">
          <span className="inbox-row-subject">{message.subject || t("（无主题）")}</span>
          <span className="inbox-row-marks">
            {message.hasAttachments && <span title={t("有附件")}>📎</span>}
            {onToggleFlag && (
              <FlagButton flagged={message.isFlagged} onToggle={() => onToggleFlag(message)} />
            )}
          </span>
        </div>
        {child && (
          <div className="inbox-row-sub">
                         {t("来自 ")}{message.accountName || message.accountEmail}
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
  onLabelClick,
}: {
  hit: SearchHit;
  selected?: boolean;
  onOpen: () => void;
  onToggleFlag?: (message: InboxMessage) => void;
  onLabelClick?: (result: GroupResult) => void;
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
          <GroupPills results={message.groupResults} onClick={onLabelClick} />
          <span className="inbox-row-time">{formatListTime(message.dateUtc)}</span>
        </div>
        <div className="inbox-row-bottom">
          <span className="inbox-row-subject">{message.subject || t("（无主题）")}</span>
          <span className="inbox-row-marks">
            {message.hasAttachments && <span title={t("有附件")}>📎</span>}
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

/** 会话分组标题：只负责显示发件方、封数和展开状态，邮件详情留给子行。 */
function ThreadRow({
  thread,
  expanded,
  onToggle,
}: {
  thread: InboxThread;
  expanded: boolean;
  onToggle: () => void;
}) {
  const sender = senderText(thread.latest);
  return (
    <div
      className="inbox-row inbox-row-thread"
      data-read={thread.unreadCount > 0 ? "false" : "true"}
      role="button"
      aria-label={`${sender} ${t("共 {0} 封", [thread.messageCount])}`}
      aria-expanded={expanded}
      onClick={onToggle}
      tabIndex={0}
      onKeyDown={(event) => handleRowKeyDown(event, onToggle)}
    >
      <svg className="thread-arrow" viewBox="0 0 14 14" aria-hidden="true" focusable="false">
        <path d="m5 3 4 4-4 4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      <span className="inbox-thread-sender" title={sender}>{sender}</span>
      <span className="thread-count">{t("共 {0} 封", [thread.messageCount])}</span>
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
  /** 系统通知被点击时，让外壳切回收件箱。 */
  onNotificationOpen?: () => void;
};

/** 统一收件箱面板。 */
export default function InboxPanel({ composeSeed, onComposeSeedConsumed, onNotificationOpen }: InboxPanelProps) {
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
  // 删除账号后，即使查询条件没变，也重新读取列表并清掉会话缓存。
  const [accountRevision, setAccountRevision] = useState(0);
  const [error, setError] = useState<string>();

  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [children, setChildren] = useState<Map<string, InboxMessage[]>>(new Map());
  const [selectedMessage, setSelectedMessage] = useState<InboxMessage>();
  const readingRequest = useRef(0);
  const [notificationError, setNotificationError] = useState<string>();
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
  // 清空后忽略尚未返回的旧搜索，避免结果重新出现。
  const searchRequest = useRef(0);
  const searchInFlight = useRef(false);
  // 搜索命令说明弹窗；输入 --help 后打开，不发起搜索请求。
  const [searchHelpOpen, setSearchHelpOpen] = useState(false);

  // 写信窗格的打开请求；为空表示当前在收件箱。
  const [composeRequest, setComposeRequest] = useState<ComposeRequest>();
  // 通知读信期间保留写信组件，避免未保存的正文丢失。
  const [readingNotification, setReadingNotification] = useState(false);
  useEffect(() => setReadingNotification(false), [composeRequest]);
  // 有没有启用的 AI 站点；只控制按钮是否可用，不触发任何请求。
  const [aiEnabled, setAiEnabled] = useState(false);
  // 已经消费过的通讯录写信请求，用来挡住重复触发。
  const consumedComposeSeed = useRef<InboxComposeSeed | undefined>(undefined);
  // 邮箱栏「添加邮箱」弹窗；保存成功后选中新账号。
  const [addAccountOpen, setAddAccountOpen] = useState(false);
  // 没有账号表示在邮箱栏空白处打开菜单。
  const [accountMenu, setAccountMenu] = useState<{ account?: AccountInboxSummary; x: number; y: number }>();
  const [deleteAccountConfirm, setDeleteAccountConfirm] = useState<AccountInboxSummary>();
  const [accountDeleteBusy, setAccountDeleteBusy] = useState(false);
  const [accountDeleteError, setAccountDeleteError] = useState("");
  // 文件夹右键菜单：落在哪个文件夹上、鼠标位置在哪。
  const [folderMenu, setFolderMenu] = useState<{ folder: InboxFolder; x: number; y: number }>();
  // 文件夹操作弹窗：新建根文件夹 / 新建子文件夹 / 重命名 / 删除。
  const [folderDialog, setFolderDialog] = useState<{
    mode: "create" | "create-child" | "rename" | "delete";
    accountId: number;
    folder?: InboxFolder;
    name: string;
  }>();
  const [folderBusy, setFolderBusy] = useState(false);
  const [folderError, setFolderError] = useState("");
  // 彻底删除一封邮件前的二次确认。
  const [deleteMessageConfirm, setDeleteMessageConfirm] = useState<InboxMessage>();
  // 邮件右键菜单与「立即分类」的一次性外发确认。
  const [messageMenu, setMessageMenu] = useState<{
    message: InboxMessage;
    x: number;
    y: number;
  }>();
  // 手动分类菜单的数据与读取状态；开菜单时只读取本地标签。
  const [menuGroups, setMenuGroups] = useState<LabelGroup[]>([]);
  const [menuGroupsLoading, setMenuGroupsLoading] = useState(false);
  const [menuGroupsError, setMenuGroupsError] = useState(false);
  const menuRequestId = useRef(0);
  const [classifyConfirm, setClassifyConfirm] = useState<{
    preview: ClassifyPreview;
    subject: string;
  }>();
  const [classifyBusy, setClassifyBusy] = useState(false);
  const [classifyError, setClassifyError] = useState("");
  // 确认框里勾了「下次不要提醒」就先记住：以后右键「立即分类」直接外发。
  const [classifySkipNext, setClassifySkipNext] = useState(false);
  // 正在编辑的账号：邮箱栏只有摘要，右键「编辑」时先取回完整账号再开弹窗。
  const [editingAccount, setEditingAccount] = useState<Account | null>(null);
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
  }, [query, listThreadMode, accountRevision]);

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

  // 邮箱栏右键菜单开着时，点别处 / 按 Esc / 一滚动就收起来。
  useEffect(() => {
    if (!accountMenu) return;
    const close = () => setAccountMenu(undefined);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    document.addEventListener("click", close);
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("click", close);
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("scroll", close, true);
    };
  }, [accountMenu]);

  /** 右键账号：在鼠标位置开菜单。 */
  const openAccountMenu = useCallback((event: ReactMouseEvent, account: AccountInboxSummary) => {
    event.preventDefault();
    event.stopPropagation();
    setFolderMenu(undefined);
    setMessageMenu(undefined);
    setAccountMenu({ account, x: event.clientX, y: event.clientY });
  }, []);

  /** 只接收邮箱栏自身的空白，避免覆盖账号、文件夹等按钮的菜单。 */
  const openSidebarMenu = useCallback((event: ReactMouseEvent) => {
    if (event.target !== event.currentTarget) return;
    event.preventDefault();
    setFolderMenu(undefined);
    setMessageMenu(undefined);
    setAccountMenu({ x: event.clientX, y: event.clientY });
  }, []);

  const openDeleteAccountDialog = useCallback((account: AccountInboxSummary) => {
    setAccountMenu(undefined);
    setAccountDeleteError("");
    setDeleteAccountConfirm(account);
  }, []);

  /** 打开账号编辑弹窗：邮箱栏只有账号摘要，先取回完整账号再开。 */
  const editAccount = useCallback(async (accountId: number) => {
    setAccountMenu(undefined);
    try {
      const list = await api.listAccounts();
      const found = list.find((item) => item.id === accountId);
      if (found) setEditingAccount(found);
    } catch (caught) {
      setError(describeError(caught));
    }
  }, []);

  // 文件夹右键菜单开着时，点别处 / 按 Esc / 一滚动就收起来。
  useEffect(() => {
    if (!folderMenu) return;
    const close = () => setFolderMenu(undefined);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    document.addEventListener("click", close);
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("click", close);
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("scroll", close, true);
    };
  }, [folderMenu]);

  /** 右键一个文件夹：在鼠标位置开菜单。 */
  const openFolderMenu = useCallback((event: ReactMouseEvent, folder: InboxFolder) => {
    event.preventDefault();
    event.stopPropagation();
    setAccountMenu(undefined);
    setMessageMenu(undefined);
    setFolderMenu({ folder, x: event.clientX, y: event.clientY });
  }, []);

  /** 开「新建文件夹」弹窗；parent 为空表示建在账号根下。 */
  const openCreateFolderDialog = useCallback((accountId: number, parent?: InboxFolder) => {
    setFolderMenu(undefined);
    setAccountMenu(undefined);
    setFolderError("");
    setFolderDialog({
      mode: parent ? "create-child" : "create",
      accountId,
      folder: parent,
      name: "",
    });
  }, []);

  /** 开重命名弹窗；输入框只预填最后一段名字。 */
  const openRenameFolderDialog = useCallback((folder: InboxFolder) => {
    setFolderMenu(undefined);
    setFolderError("");
    const leaf = folder.delimiter
      ? folder.fullPath.split(folder.delimiter).pop() ?? folder.fullPath
      : folder.fullPath;
    setFolderDialog({
      mode: "rename",
      accountId: folder.accountId,
      folder,
      name: leaf,
    });
  }, []);

  /** 开删除文件夹确认弹窗。 */
  const openDeleteFolderDialog = useCallback((folder: InboxFolder) => {
    setFolderMenu(undefined);
    setFolderError("");
    setFolderDialog({
      mode: "delete",
      accountId: folder.accountId,
      folder,
      name: folder.fullPath,
    });
  }, []);

  /** 提交文件夹操作：先落服务器，成功后再刷新侧栏。 */
  const submitFolderDialog = useCallback(async () => {
    if (!folderDialog) return;
    const name = folderDialog.name.trim();
    if (folderDialog.mode !== "delete" && !name) {
      setFolderError(t("文件夹名字不能为空"));
      return;
    }
    setFolderBusy(true);
    setFolderError("");
    try {
      if (folderDialog.mode === "create") {
        await api.createMailFolder(folderDialog.accountId, undefined, name);
      } else if (folderDialog.mode === "create-child") {
        await api.createMailFolder(folderDialog.accountId, folderDialog.folder?.folderId, name);
      } else if (folderDialog.mode === "rename" && folderDialog.folder) {
        await api.renameMailFolder(folderDialog.folder.folderId, name);
      } else if (folderDialog.mode === "delete" && folderDialog.folder) {
        await api.deleteMailFolder(folderDialog.folder.folderId);
      }
      setFolderDialog(undefined);
      setStatusNote(
        folderDialog.mode === "delete" ? t("文件夹已删除") : t("文件夹已同步到服务器"),
      );
      await refreshSidebar();
    } catch (caught) {
      setFolderError(describeError(caught));
      // 服务商拒绝后本地可能记下了「这里不能再分层」，刷新一下让右键菜单同步。
      if (folderDialog.mode === "create-child") {
        await refreshSidebar();
      }
    } finally {
      setFolderBusy(false);
    }
  }, [folderDialog, refreshSidebar]);

  // 邮件右键菜单开着时，点别处 / 按 Esc / 一滚动就收起来。
  useEffect(() => {
    if (!messageMenu) return;
    const close = () => setMessageMenu(undefined);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    const onScroll = (event: Event) => {
      if (event.target instanceof Element && event.target.closest(".classification-submenu")) return;
      close();
    };
    document.addEventListener("click", close);
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("click", close);
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
    };
  }, [messageMenu]);

  /** 右键一封邮件：在鼠标位置开菜单；只对点中的那一封生效。 */
  const openMessageMenu = useCallback((event: ReactMouseEvent, message: InboxMessage) => {
    event.preventDefault();
    event.stopPropagation();
    setAccountMenu(undefined);
    setFolderMenu(undefined);
    const requestId = ++menuRequestId.current;
    setMenuGroups([]);
    setMenuGroupsLoading(true);
    setMenuGroupsError(false);
    setMessageMenu({ message, x: event.clientX, y: event.clientY });
    void api.listLabelGroups()
      .then((groups) => {
        if (requestId === menuRequestId.current) setMenuGroups(groups);
      })
      .catch(() => {
        if (requestId === menuRequestId.current) setMenuGroupsError(true);
      })
      .finally(() => {
        if (requestId === menuRequestId.current) setMenuGroupsLoading(false);
      });
  }, []);

  /** 从邮件右键菜单打开回复或转发，并关闭菜单。 */
  const composeFromMessage = useCallback((kind: "reply" | "forward", message: InboxMessage) => {
    setMessageMenu(undefined);
    setComposeRequest({ kind, sourceMessageId: message.id });
  }, []);

  /**
   * 点「立即分类」：先拿外发预览。没开「下次不要提醒」就弹一次确认；
   * 开了就按用户偏好直接外发，不再逐封确认。取消时不外发、不写分类结果，但正文本地没有时可能已先下载到本机。
   */
  const startClassify = useCallback(async (message: InboxMessage) => {
    setMessageMenu(undefined);
    setClassifyError("");
    setClassifyBusy(true);
    try {
      const [settings, preview] = await Promise.all([
        api.getAppSettings(),
        api.previewClassifyMessages([message.id]),
      ]);
      if (settings.classifySkipManualConfirm) {
        await api.classifyMessages(preview.authorizationToken);
        await load(true);
      } else {
        setClassifySkipNext(false);
        setClassifyConfirm({ preview, subject: message.subject || t("（无主题）") });
      }
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setClassifyBusy(false);
    }
  }, [load]);

  /**
   * 确认框里点「确认分类」：这时才真正外发；勾了「下次不要提醒」就先记住这个偏好。
   * 完成后刷新列表拿新标签。
   */
  const confirmClassify = useCallback(async () => {
    if (!classifyConfirm) return;
    setClassifyBusy(true);
    setClassifyError("");
    try {
      if (classifySkipNext) {
        await api.setClassifySkipManualConfirm(true);
      }
      await api.classifyMessages(classifyConfirm.preview.authorizationToken);
      setClassifyConfirm(undefined);
      setClassifySkipNext(false);
      await load(true);
    } catch (caught) {
      setClassifyError(describeError(caught));
    } finally {
      setClassifyBusy(false);
    }
  }, [classifyConfirm, classifySkipNext, load]);

  /** 右键菜单里点某组的某个值：写入手动结果（该组从此锁定），再刷新列表。 */
  const setGroupValue = useCallback(
    async (groupId: number, labelId: number | null, scoreValue: number | null) => {
      if (!messageMenu) return;
      const messageId = messageMenu.message.id;
      setMessageMenu(undefined);
      try {
        await api.setMessageGroupValue(messageId, groupId, labelId, scoreValue);
        await load(true);
      } catch (caught) {
        setError(describeError(caught));
      }
    },
    [messageMenu, load],
  );

  /** 右键菜单里点「清除本组结果」：删掉该组结果并解锁，再刷新列表。 */
  const clearGroupValue = useCallback(
    async (groupId: number) => {
      if (!messageMenu) return;
      const messageId = messageMenu.message.id;
      setMessageMenu(undefined);
      try {
        await api.clearMessageGroupValue(messageId, groupId);
        await load(true);
      } catch (caught) {
        setError(describeError(caught));
      }
    },
    [messageMenu, load],
  );

  /**
   * 搜索：默认只查本地，绝不自动联网。
   * deep=true 才允许联网补历史；当前搜索栏不提供该入口。
   * append=true 表示继续加载未显示的结果，追加到已有列表。
   */
  const runSearch = useCallback(
    async (options?: { deep?: boolean; append?: boolean; raw?: string }) => {
      if (options?.append && searchInFlight.current) return;
      const raw = (options?.raw ?? searchInput).trim();
      if (raw.toLowerCase() === "--help") {
        setSearchHelpOpen(true);
        setSearchError(undefined);
        return;
      }
      const request = ++searchRequest.current;
      if (raw === "") {
        searchInFlight.current = false;
        setSearching(false);
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
      searchInFlight.current = true;
      setSearching(true);
      setSearchError(undefined);
      try {
        const page = await api.searchMessages({
          raw,
          ...(selectedAccount === undefined ? {} : { accountId: selectedAccount }),
          ...(offset > 0 ? { offset } : {}),
          limit: PAGE_SIZE,
          deep,
        });
        if (request !== searchRequest.current) return;
        setSearchRaw(raw);
        setHits((old) => (append ? [...old, ...page.items] : page.items));
        setSearchTotal(page.total);
        setSearchNote(
          page.deepSynced
            ? t("已联网补拉一批历史")
            : page.deepError
              ? t("补拉历史失败：{0}", [page.deepError])
              : "",
        );
      } catch (caught) {
        if (request === searchRequest.current) setSearchError(describeError(caught));
      } finally {
        if (request === searchRequest.current) {
          searchInFlight.current = false;
          setSearching(false);
        }
      }
    },
    [searchInput, selectedAccount, hits.length],
  );

  /** 搜索结果滚到列表底部附近时，继续加载未显示的结果。 */
  const loadMoreSearch = useCallback(() => {
    if (searchRaw === "" || searching || hits.length >= searchTotal) return;
    const element = scrollRef.current;
    if (!element) return;
    const distanceToBottom = element.scrollHeight - element.scrollTop - element.clientHeight;
    if (distanceToBottom > 240) return;
    void runSearch({ append: true, raw: searchRaw });
  }, [searchRaw, searching, hits.length, searchTotal, runSearch]);

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

  /** 关闭搜索命令说明并把焦点送回搜索框。 */
  const closeSearchHelp = useCallback(() => {
    setSearchHelpOpen(false);
    searchBoxRef.current?.focus();
  }, []);

  /** 搜索命令弹窗打开时，按 Esc 关闭。 */
  useEffect(() => {
    if (!searchHelpOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeSearchHelp();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeSearchHelp, searchHelpOpen]);

  /** 点列表里的胶囊：按 tag:组名:值 搜一次。 */
  const filterByLabel = useCallback(
    (result: GroupResult) => {
      const value =
        result.kind === "score" ? String(result.scoreValue ?? "") : (result.labelName ?? "");
      const raw = `tag:${result.groupName}:${value}`;
      setSearchInput(raw);
      void runSearch({ raw });
    },
    [runSearch],
  );

  /** 退出搜索，回到普通收件箱列表。 */
  const clearSearch = useCallback(() => {
    searchRequest.current += 1;
    searchInFlight.current = false;
    setSearching(false);
    setSearchInput("");
    setSearchRaw("");
    setHits([]);
    setSearchTotal(0);
    setSearchNote("");
    setSearchError(undefined);
  }, []);

  /** 确认后停止该账号同步，再复用现有本机删除功能。 */
  const confirmDeleteAccount = useCallback(async () => {
    if (!deleteAccountConfirm || accountDeleteBusy) return;
    const accountId = deleteAccountConfirm.accountId;
    setAccountDeleteBusy(true);
    setAccountDeleteError("");
    try {
      await api.stopSync(accountId);
      await api.deleteAccount(accountId);
      if (selectedAccount === accountId) {
        setSelectedAccount(undefined);
        setSelectedFolder(undefined);
        setFlaggedView(false);
        setUnifiedView("inbox");
      }
      setSelectedMessage((current) => current?.accountId === accountId ? undefined : current);
      setExpandedAccounts((current) => {
        const next = new Set(current);
        next.delete(accountId);
        return next;
      });
      setSyncStatuses((current) => current.filter((item) => item.accountId !== accountId));
      clearSearch();
      setAccountRevision((current) => current + 1);
      setDeleteAccountConfirm(undefined);
      setStatusNote(t("已删除账号「{0}」。", [deleteAccountConfirm.displayName || deleteAccountConfirm.email]));
      await refreshSidebar();
    } catch (caught) {
      setAccountDeleteError(describeError(caught));
    } finally {
      setAccountDeleteBusy(false);
    }
  }, [deleteAccountConfirm, accountDeleteBusy, selectedAccount, clearSearch, refreshSidebar]);

  const searchMode = searchRaw !== "";

  const rows = useMemo(
    () =>
      searchMode
        ? searchRows(hits)
        : flattenInboxRows(threads, messages, listThreadMode, expanded, children),
    [searchMode, hits, threads, messages, listThreadMode, expanded, children],
  );

  // 普通邮件和搜索结果使用各自的行键，避免按序号复用不同内容的高度。
  const getRowKey = useCallback((index: number) => rows[index]?.key ?? index, [rows]);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getItemKey: getRowKey,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => {
      const row = rows[index];
      return row ? inboxRowHeight(row.kind) : 73;
    },
    overscan: 8,
  });

  const virtualItems = virtualizer.getVirtualItems();

  /** 普通列表按可见行补充邮件；搜索结果由列表滚动事件继续加载。 */
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

  /** 点「删除」：先问后端会走哪条路；彻底删除才弹二次确认。 */
  const startDeleteMessage = useCallback(
    async (message: InboxMessage) => {
      setMessageMenu(undefined);
      try {
        const preview = await api.previewDeleteMessage(message.id);
        if (preview.permanent) {
          setDeleteMessageConfirm(message);
          return;
        }
        await api.deleteMessage(message.id, false);
        setStatusNote(t("邮件已移到「已删除」"));
        await refreshAll();
      } catch (caught) {
        setError(describeError(caught));
      }
    },
    [refreshAll],
  );

  /** 二次确认后彻底删除一封邮件。 */
  const confirmDeleteMessage = useCallback(async () => {
    if (!deleteMessageConfirm) return;
    setFolderBusy(true);
    try {
      await api.deleteMessage(deleteMessageConfirm.id, true);
      setDeleteMessageConfirm(undefined);
      setStatusNote(t("邮件已彻底删除"));
      await refreshAll();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setFolderBusy(false);
    }
  }, [deleteMessageConfirm, refreshAll]);

  /** 打开邮件：本地先乐观标已读，失败回滚并提示；未读筛选下移出列表。 */
  const openMessage = useCallback(
    async (message: InboxMessage) => {
      const request = ++readingRequest.current;
      setNotificationError(undefined);
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
        if (request === readingRequest.current && unreadOnly) setStatusNote(t("已标为已读，已移出「只看未读」列表"));
      } catch (caught) {
        if (request === readingRequest.current) {
          setSelectedMessage(message);
          setError(t("标记已读失败：{0}", [describeError(caught)]));
        }
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
              ? t("已在本机标红，服务器同步失败，稍后会自动重试")
              : t("已在本机取消标红，服务器同步失败，稍后会自动重试"),
          );
        }
        // 红旗视图里取消标红，这一行就该走人，和「只看未读」一个道理。
        if (effectiveFlagged && !next) {
          setMessages((old) => old.filter((item) => item.id !== message.id));
          setTotal((old) => Math.max(0, old - 1));
          if (result.synced) setStatusNote(t("已取消标红，已移出「红旗邮件」"));
        }
      } catch (caught) {
        // 本地都没写成功：回滚到点击前的状态，别让界面骗人。
        applyMessageFlag(message.id, message.isFlagged);
        setStatusNote(t("切换红旗失败：{0}", [describeError(caught)]));
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

  // 回调跟着当前筛选更新；事件本身只订阅一次，避免刷新时出现监听空档。
  const notificationHandlers = useRef({ openMessage, onNotificationOpen });
  useEffect(() => {
    notificationHandlers.current = { openMessage, onNotificationOpen };
  }, [openMessage, onNotificationOpen]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    let latestRequest = 0;
    const start = async () => {
      try {
        const off = await listen<{ messageId: number }>("inbox:open-message", (event) => {
          const messageId = event.payload?.messageId;
          if (!Number.isSafeInteger(messageId) || messageId <= 0) return;
          const request = ++latestRequest;
          setNotificationError(undefined);
          notificationHandlers.current.onNotificationOpen?.();
          void (async () => {
            try {
              const message = await api.getInboxMessage(messageId);
              if (cancelled || request !== latestRequest) return;
              if (!message) {
                setNotificationError(t("这封邮件已不存在，可能已被删除或重新同步。"));
                return;
              }
              setReadingNotification(true);
              setError(undefined);
              await notificationHandlers.current.openMessage(message);
            } catch (caught) {
              if (!cancelled && request === latestRequest) setNotificationError(describeError(caught));
            }
          })();
        });
        if (cancelled) off();
        else unlisten = off;
      } catch {
        // 单元测试与浏览器预览里没有 Tauri 事件总线，忽略即可。
      }
    };
    void start();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

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

  /** 统一入口统计所有账号，选中账号后只统计该账号。 */
  const currentSyncStatuses = useMemo(
    // 同步管理器会保留已停止的状态；删除后的账号不再参与数量合计。
    () => syncStatuses.filter((item) =>
      summary?.accounts.some((account) => account.accountId === item.accountId)
      && (selectedAccount === undefined || item.accountId === selectedAccount),
    ),
    [syncStatuses, selectedAccount, summary],
  );
  const currentSync = worstSyncStatus(currentSyncStatuses);

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
  const menuAccount = accountMenu?.account;

  return (
    <>
    <div
      className="inbox-layout"
      style={{
        gridTemplateColumns: `${widths.sidebar}px ${RESIZER_WIDTH}px ${widths.list}px ${RESIZER_WIDTH}px minmax(0, 1fr)`,
      }}
    >
      <aside className="inbox-sidebar" aria-label={t("邮箱栏")} onContextMenu={openSidebarMenu}>
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
              <span className="sidebar-item-title">{t(view.label)}</span>
              {badge > 0 && <span className="unread-badge">{badge}</span>}
            </button>
          );
        })}

        {accounts.map((account) => (
          <div key={account.accountId} className="sidebar-account">
            <button
              type="button"
              onContextMenu={(event) => openAccountMenu(event, account)}
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
                      title={t("这个账号里所有标红的邮件")}
                      onClick={() => {
                        setSelectedAccount(account.accountId);
                        setSelectedFolder(undefined);
                        setFlaggedView(true);
                      }}
                    >
                      <span className="sidebar-item-title">{t("红旗邮件")}</span>
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
                      onContextMenu={(event) => openFolderMenu(event, entry.folder)}
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
          <span className="sidebar-item-title">{t("＋ 添加邮箱")}</span>
        </button>
      </aside>

      <PaneResizer
        label={t("邮箱栏宽度")}
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
            {t("写邮件")}</button>
          {composeRequest && readingNotification && (
            <button type="button" onClick={() => setReadingNotification(false)}>
              {t("继续写信")}
            </button>
          )}
          <button
            type="button"
            className={threadMode ? "inbox-icon-button is-active" : "inbox-icon-button"}
            aria-label={t("按会话聚合")}
            aria-pressed={threadMode}
            title={t("按会话聚合")}
            disabled={effectiveFlagged}
            onClick={() => setThreadMode((current) => !current)}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
              {threadMode ? (
                <>
                  <path d="M7 4v16" />
                  <path d="M7 6h4a3 3 0 0 1 3 3v8" />
                  <path d="M7 12h4a3 3 0 0 1 3 3v2" />
                  <path d="M17 7h3M17 12h3M17 17h3" />
                </>
              ) : (
                <>
                  <path d="M5 5h14M5 10h14M5 15h14M5 20h14" />
                  <path d="M3 5h.01M3 10h.01M3 15h.01M3 20h.01" />
                </>
              )}
            </svg>
          </button>
          <button
            type="button"
            className={effectiveUnread ? "inbox-icon-button is-active" : "inbox-icon-button"}
            aria-label={t("只看未读")}
            aria-pressed={effectiveUnread}
            title={
              inUnified && unifiedView === "unread"
                ? t("「所有未读」已经只看未读了")
                : t("只看未读")
            }
            disabled={inUnified && unifiedView === "unread"}
            onClick={() => setUnreadOnly((current) => !current)}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
              <rect x="3" y="5" width="18" height="14" rx="2" />
              <path d="m4 7 8 6 8-6" />
              {effectiveUnread && <circle cx="19" cy="5" r="2.5" fill="currentColor" stroke="none" />}
            </svg>
          </button>
          <button type="button" onClick={() => void refreshAll()} disabled={loading}>
            {t("刷新")}</button>
          {currentSync && (
            <span
              className={syncBadgeClass(currentSync)}
              tabIndex={0}
              role="status"
              title={currentSyncStatuses.map(syncBadgeTitle).join("\n")}
            >
              {syncBadgeText(currentSync, currentSyncStatuses)}
            </span>
          )}
          {currentSync && (currentSync.needsReauth || currentSync.state === "error") && (
            <button type="button" onClick={() => void retrySync()}>
              {t("重试")}</button>
          )}
          {(loading || searching) && <span className="hint">{t("加载中……")}</span>}
        </div>

        <div className="inbox-search">
          <div className="inbox-search-field">
            <svg
              className="inbox-search-icon"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.75"
              strokeLinecap="round"
              aria-hidden="true"
              focusable="false"
            >
              <circle cx="10.5" cy="10.5" r="7" />
              <path d="m16 16 5 5" />
            </svg>
            <input
              ref={searchBoxRef}
              aria-label={t("搜索邮件")}
              aria-keyshortcuts="Control+K Meta+K"
              aria-busy={searching}
              title={t("输入 --help 查看命令使用方法")}
              value={searchInput}
              placeholder={t(SEARCH_PLACEHOLDER)}
              onChange={(event) => setSearchInput(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  // 帮助弹窗会自动聚焦“知道了”；拦住同一次回车继续点击它。
                  event.preventDefault();
                  void runSearch();
                }
                if (event.key === "Escape" && searchMode) clearSearch();
              }}
            />
            {searchInput.length > 0 ? (
              <button
                type="button"
                className="inbox-search-clear"
                aria-label={t("清空搜索")}
                title={t("清空搜索")}
                onClick={() => {
                  clearSearch();
                  searchBoxRef.current?.focus();
                }}
              >
                <svg
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinecap="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <path d="m6 6 12 12M18 6 6 18" />
                </svg>
              </button>
            ) : (
              <kbd className="inbox-search-shortcut" aria-hidden="true">Ctrl+K</kbd>
            )}
          </div>
        </div>

        {notificationError && <p className="error" role="alert">{notificationError}</p>}
        {error && (
          <p className="error" role="alert">
            {t("加载失败：")}{error}
          </p>
        )}
        {searchError && (
          <p className="error" role="alert">
            {t("搜索失败：")}{searchError}
          </p>
        )}
        {searchNote && <p className="hint inbox-search-note">{searchNote}</p>}
        {statusNote && (
          <p className="hint inbox-status-note" role="status">
            {statusNote}
          </p>
        )}
        <div className="inbox-scroll" ref={scrollRef} onScroll={loadMoreSearch}>
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
                  data-index={item.index}
                  ref={virtualizer.measureElement}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${item.start}px)`,
                    minHeight: inboxRowHeight(row.kind),
                  }}
                  onContextMenu={(event) =>
                    openMessageMenu(
                      event,
                      row.kind === "thread"
                        ? row.thread.latest
                        : row.kind === "search"
                          ? row.hit.message
                          : row.message,
                    )
                  }
                >
                  {row.kind === "thread" ? (
                    <ThreadRow
                      thread={row.thread}
                      expanded={expanded.has(row.key)}
                      onToggle={() => void toggleThread(row.thread)}
                    />
                  ) : row.kind === "search" ? (
                    <SearchResultRow
                      hit={row.hit}
                      selected={selectedMessage?.id === row.hit.message.id}
                      onOpen={() => void openMessage(row.hit.message)}
                      onToggleFlag={(message) => void toggleFlag(message)}
                      onLabelClick={filterByLabel}
                    />
                  ) : (
                    <MessageRow
                      message={row.message}
                      child={row.kind === "thread-message"}
                      selected={selectedMessage?.id === row.message.id}
                      onOpen={() => void openMessage(row.message)}
                      onToggleFlag={(message) => void toggleFlag(message)}
                      onLabelClick={filterByLabel}
                    />
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </section>

      <PaneResizer
        label={t("邮件列表宽度")}
        value={widths.list}
        min={paneLimits.list.min}
        max={paneLimits.list.max}
        onChange={setList}
        onReset={resetPaneWidths}
        onCommit={persistPaneWidths}
      />

      <aside className="inbox-reader">
        {composeRequest && (
          <ComposePanel
            request={composeRequest}
            accounts={accounts}
            onClose={() => setComposeRequest(undefined)}
            onSent={() => void refreshAll()}
            aiEnabled={aiEnabled}
            active={!readingNotification}
          />
        )}
        {(!composeRequest || readingNotification) && (
          <MessageReader
            message={selectedMessage}
            aiEnabled={aiEnabled}
            onToggleFlag={(message) => void toggleFlag(message)}
            onLabelChanged={() => void load(true)}
          />
        )}
      </aside>
    </div>
    {accountMenu && (
      <div
        className="context-menu"
        role="menu"
        aria-label={t("邮箱菜单")}
        style={{
          left: Math.min(accountMenu.x, Math.max(8, window.innerWidth - 220)),
          top: Math.min(accountMenu.y, Math.max(8, window.innerHeight - (menuAccount ? 140 : 60))),
        }}
        onClick={(event) => event.stopPropagation()}
      >
        {menuAccount ? (
          <>
            <button
              type="button"
              role="menuitem"
              className="context-menu-item"
              onClick={() => void editAccount(menuAccount.accountId)}
            >
              {t("编辑")}</button>
            <button
              type="button"
              role="menuitem"
              className="context-menu-item"
              onClick={() => openCreateFolderDialog(menuAccount.accountId)}
            >
              {t("新建文件夹")}</button>
            <button
              type="button"
              role="menuitem"
              className="context-menu-item danger"
              onClick={() => openDeleteAccountDialog(menuAccount)}
            >
              {t("删除邮箱")}</button>
          </>
        ) : (
          <button
            type="button"
            role="menuitem"
            className="context-menu-item"
            onClick={() => {
              setAccountMenu(undefined);
              setAddAccountOpen(true);
            }}
          >
            {t("添加邮箱")}</button>
        )}
      </div>
    )}

    {folderMenu && (
      <div
        className="context-menu"
        role="menu"
        aria-label={t("文件夹菜单")}
        style={{
          left: Math.min(folderMenu.x, Math.max(8, window.innerWidth - 240)),
          top: Math.min(folderMenu.y, Math.max(8, window.innerHeight - 170)),
        }}
        onClick={(event) => event.stopPropagation()}
      >
        {folderMenu.folder.allowsChildren && (
          <button
            type="button"
            role="menuitem"
            className="context-menu-item"
            onClick={() => openCreateFolderDialog(folderMenu.folder.accountId, folderMenu.folder)}
          >
            {t("新建子文件夹")}</button>
        )}
        {folderMenu.folder.childCreateBlocked && (
          <button type="button" role="menuitem" className="context-menu-item" disabled>
            {t("服务商不允许在此文件夹下建子文件夹")}</button>
        )}
        {folderMenu.folder.changeable && (
          <button
            type="button"
            role="menuitem"
            className="context-menu-item"
            onClick={() => openRenameFolderDialog(folderMenu.folder)}
          >
            {t("重命名")}</button>
        )}
        {folderMenu.folder.changeable && (
          <button
            type="button"
            role="menuitem"
            className="context-menu-item danger"
            onClick={() => openDeleteFolderDialog(folderMenu.folder)}
          >
            {t("删除文件夹")}</button>
        )}
        {!folderMenu.folder.changeable && (
          <button type="button" role="menuitem" className="context-menu-item" disabled>
            {t("系统文件夹不支持改名或删除")}</button>
        )}
      </div>
    )}

    {messageMenu && (
      <div
        className="context-menu"
        role="menu"
        aria-label={t("邮件菜单")}
        style={{
          left: Math.min(messageMenu.x, Math.max(8, window.innerWidth - 220)),
          top: Math.min(messageMenu.y, Math.max(8, window.innerHeight - 220)),
        }}
        onClick={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          role="menuitem"
          className="context-menu-item"
          onClick={() => composeFromMessage("reply", messageMenu.message)}
        >
          {t("回复")}</button>
        <button
          type="button"
          role="menuitem"
          className="context-menu-item"
          onClick={() => composeFromMessage("forward", messageMenu.message)}
        >
          {t("转发")}</button>
        <button
          type="button"
          role="menuitem"
          className="context-menu-item"
          onClick={() => void startClassify(messageMenu.message)}
          disabled={classifyBusy}
        >
          {t("立即分类")}</button>
        <MessageClassificationMenu
          groups={menuGroups}
          loading={menuGroupsLoading}
          error={menuGroupsError}
          onSetValue={(groupId, labelId, scoreValue) => void setGroupValue(groupId, labelId, scoreValue)}
          onClearValue={(groupId) => void clearGroupValue(groupId)}
        />
        <button
          type="button"
          role="menuitem"
          className="context-menu-item danger"
          onClick={() => void startDeleteMessage(messageMenu.message)}
        >
          {t("删除")}</button>
      </div>
    )}

    {searchHelpOpen && (
      <div
        className="ai-modal-backdrop"
        role="presentation"
        onMouseDown={(event) => {
          if (event.target === event.currentTarget) closeSearchHelp();
        }}
      >
        <section
          className="ai-modal search-help-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t("搜索命令")}
        >
          <h3>{t("搜索命令")}</h3>
          <p>{t("输入关键词，或把下面命令和关键词用空格组合起来。")}</p>
          <dl className="search-help-commands">
            <div>
              <dt><code>{t("关键词")}</code></dt>
              <dd>{t("搜索主题、发件人和正文。")}</dd>
            </div>
            <div>
              <dt><code>{t("from:发件人")}</code></dt>
              <dd>{t("只看某个发件人，例如 from:alice。")}</dd>
            </div>
            <div>
              <dt><code>has:attachment</code></dt>
              <dd>{t("只看带附件的邮件。")}</dd>
            </div>
            <div>
              <dt><code>is:unread</code></dt>
              <dd>{t("只看未读邮件。")}</dd>
            </div>
            <div>
              <dt><code>before:YYYY-MM-DD</code></dt>
              <dd>{t("只看这个日期以前的邮件。")}</dd>
            </div>
            <div>
              <dt><code>{t("tag:组名:值")}</code></dt>
              <dd>{t("按本地标签筛选，例如 tag:内容分类:账单。")}</dd>
            </div>
          </dl>
          <p className="hint">{t("多个条件用空格连接，需要同时满足。")}</p>
          <div className="form-actions">
            <button type="button" className="primary" autoFocus onClick={closeSearchHelp}>
              {t("知道了")}
            </button>
          </div>
        </section>
      </div>
    )}

    {classifyConfirm && (
      <div className="ai-modal-backdrop" role="presentation">
        <section
          className="ai-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t("确认立即分类")}
        >
          <h3>{t("确认立即分类")}</h3>
          <p>{t("这封邮件的正文会发给下面的目标做一次分类，确认后才会外发。")}</p>
          <dl className="ai-modal-targets">
            <dt>{t("目标邮件")}</dt>
            <dd>{classifyConfirm.subject}</dd>
            <dt>{t("站点")}</dt>
            <dd>{classifyConfirm.preview.providerLabel}</dd>
            <dt>{t("完整域名")}</dt>
            <dd>{classifyConfirm.preview.host}</dd>
            <dt>{t("用哪个模型")}</dt>
            <dd>{classifyConfirm.preview.model}</dd>
            <dt>{t("是不是本地服务")}</dt>
            <dd>
              {classifyConfirm.preview.local
                ? t("是，本地服务，内容不离开这台电脑")
                : t("不是，内容会发送到远程站点")}
            </dd>
          </dl>
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label={t("下次不要提醒")}
              checked={classifySkipNext}
              onChange={(event) => setClassifySkipNext(event.target.checked)}
              disabled={classifyBusy}
            />
            <span>{t("下次不要提醒")}</span>
          </label>
          {classifyError && (
            <p className="error" role="alert">
              {t("操作失败：")}{classifyError}
            </p>
          )}
          <div className="form-actions">
            <button
              type="button"
              className="primary"
              onClick={() => void confirmClassify()}
              disabled={classifyBusy}
            >
              {t("确认分类")}</button>
            <button
              type="button"
              onClick={() => setClassifyConfirm(undefined)}
              disabled={classifyBusy}
            >
              {t("取消")}</button>
          </div>
        </section>
      </div>
    )}

    {folderDialog && (
      <div className="ai-modal-backdrop" role="presentation">
        <section
          className="ai-modal"
          role="dialog"
          aria-modal="true"
          aria-label={folderDialog.mode === "delete" ? t("确认删除文件夹") : t("文件夹")}
        >
          <h3>
            {folderDialog.mode === "create"
              ? t("新建文件夹")
              : folderDialog.mode === "create-child"
                ? t("新建子文件夹")
                : folderDialog.mode === "rename"
                  ? t("重命名文件夹")
                  : t("确认删除文件夹")}
          </h3>
          {folderDialog.mode === "delete" ? (
            <p>
              {t("确定要删除「{0}」吗？文件夹里还有邮件或子文件夹时服务器会拒绝。", [
                folderDialog.folder ? folderLabel(folderDialog.folder) : folderDialog.name,
              ])}
            </p>
          ) : (
            <>
              {folderDialog.mode === "create-child" && folderDialog.folder && (
                <p>{t("建在「{0}」下面。", [folderLabel(folderDialog.folder)])}</p>
              )}
              <label className="field">
                <span>{t("文件夹名字")}</span>
                <input
                  autoFocus
                  value={folderDialog.name}
                  onChange={(event) =>
                    setFolderDialog((current) =>
                      current ? { ...current, name: event.target.value } : current,
                    )
                  }
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && !folderBusy) void submitFolderDialog();
                  }}
                  disabled={folderBusy}
                />
              </label>
            </>
          )}
          {folderError && (
            <p className="error" role="alert">
              {t("操作失败：")}{folderError}
            </p>
          )}
          <div className="form-actions">
            <button
              type="button"
              className="primary"
              onClick={() => void submitFolderDialog()}
              disabled={folderBusy}
            >
              {folderDialog.mode === "delete" ? t("确认删除") : t("保存")}</button>
            <button
              type="button"
              onClick={() => setFolderDialog(undefined)}
              disabled={folderBusy}
            >
              {t("取消")}</button>
          </div>
        </section>
      </div>
    )}

    {deleteAccountConfirm && (
      <div className="ai-modal-backdrop" role="presentation">
        <section
          className="ai-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t("确认删除邮箱")}
          aria-busy={accountDeleteBusy}
          onKeyDown={(event) => {
            if (event.key === "Escape" && !accountDeleteBusy) {
              event.preventDefault();
              setDeleteAccountConfirm(undefined);
            }
            if (event.key === "Tab") {
              const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not([disabled])"));
              if (buttons.length === 0) return;
              const next = event.shiftKey ? buttons[0] : buttons[buttons.length - 1];
              if (document.activeElement === next) {
                event.preventDefault();
                (event.shiftKey ? buttons[buttons.length - 1] : buttons[0]).focus();
              }
            }
          }}
        >
          <h3>{t("确认删除邮箱")}</h3>
          <p>{t("确定删除邮箱「{0}」吗？", [deleteAccountConfirm.displayName || deleteAccountConfirm.email])}</p>
          <p>{t("将移除本机账号、关联邮件数据和保存的凭据，不会删除服务器上的邮件。")}</p>
          {accountDeleteError && <p className="error" role="alert">{t("操作失败：")}{accountDeleteError}</p>}
          <div className="form-actions">
            <button type="button" className="danger" onClick={() => void confirmDeleteAccount()} disabled={accountDeleteBusy}>
              {accountDeleteBusy ? t("删除中……") : t("确认删除")}</button>
            <button type="button" autoFocus onClick={() => setDeleteAccountConfirm(undefined)} disabled={accountDeleteBusy}>
              {t("取消")}</button>
          </div>
        </section>
      </div>
    )}

    {deleteMessageConfirm && (
      <div className="ai-modal-backdrop" role="presentation">
        <section
          className="ai-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t("确认彻底删除邮件")}
        >
          <h3>{t("确认彻底删除邮件")}</h3>
          <p>{t("服务器上会真的删除这封邮件，找不回来。")}</p>
          <p className="muted">{deleteMessageConfirm.subject || t("（无主题）")}</p>
          <div className="form-actions">
            <button
              type="button"
              className="primary"
              onClick={() => void confirmDeleteMessage()}
              disabled={folderBusy}
            >
              {t("彻底删除")}</button>
            <button
              type="button"
              onClick={() => setDeleteMessageConfirm(undefined)}
              disabled={folderBusy}
            >
              {t("取消")}</button>
          </div>
        </section>
      </div>
    )}

    <AccountDialog
      open={addAccountOpen}
      source="mailbox"
      account={null}
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

    <AccountDialog
      open={editingAccount !== null}
      source="mailbox"
      account={editingAccount}
      onClose={() => setEditingAccount(null)}
      onSaved={() => {
        setEditingAccount(null);
        setStatusNote(t("账号已更新。"));
        void refreshSidebar();
      }}
    />
    </>
  );
}
