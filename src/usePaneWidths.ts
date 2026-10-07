import { useCallback, useEffect, useMemo, useRef, useState } from "react";

/** 最左侧设置栏宽度，固定不参与拖动。 */
export const RAIL_WIDTH = 60;
/** 拖动条可点击宽度，太细不好点。 */
export const RESIZER_WIDTH = 8;
/** 邮件内容栏最小宽度，任何窗口尺寸下都不许低于这个值。 */
export const READER_MIN_WIDTH = 360;
/** 写信工作区最小宽度，比普通阅读区更宽。 */
export const COMPOSE_MIN_WIDTH = 480;

/** 一条栏的宽度上下限。 */
export type PaneWidthLimits = { min: number; max: number };

/** 收件箱那两栏的上下限（老用户宽度记忆沿用这套，不许改）。 */
export const SIDEBAR_LIMITS: PaneWidthLimits = { min: 180, max: 320 };
export const LIST_LIMITS: PaneWidthLimits = { min: 300, max: 560 };

export type PaneWidths = {
  /** 邮箱栏宽度。 */
  sidebar: number;
  /** 邮件列表宽度。 */
  list: number;
};

export const DEFAULT_PANE_WIDTHS: PaneWidths = { sidebar: 240, list: 420 };

/**
 * 一套「两栏 + 内容区」的宽度档位。
 *
 * 收件箱和通讯录用的是同一套拖动逻辑，但存储键、默认值和上下限各有一套，
 * 所以宽度按档位分开存、分开算，互不影响。
 */
export type PaneProfile = {
  /** 本地存储键；每套档位一个，不能和别人撞。 */
  storageKey: string;
  /** 第一次打开时的默认宽度。 */
  defaults: PaneWidths;
  /** 左栏上下限。 */
  sidebarLimits: PaneWidthLimits;
  /** 中栏上下限。 */
  listLimits: PaneWidthLimits;
};

/** 收件箱档位（历史行为，一个字都不许变）。 */
export const INBOX_PANE_PROFILE: PaneProfile = {
  storageKey: "ymail.pane-widths.v1",
  defaults: DEFAULT_PANE_WIDTHS,
  sidebarLimits: SIDEBAR_LIMITS,
  listLimits: LIST_LIMITS,
};

/** 通讯录档位：左「分组栏」、中「列表栏」。 */
export const CONTACTS_PANE_PROFILE: PaneProfile = {
  storageKey: "ymail.contacts-pane-widths.v1",
  defaults: { sidebar: 220, list: 360 },
  sidebarLimits: { min: 180, max: 320 },
  listLimits: { min: 300, max: 560 },
};

function clamp(value: number, min: number, max: number): number {
  if (Number.isNaN(value)) return min;
  return Math.min(max, Math.max(min, value));
}

/** 本地宽度记忆：读失败就回默认值，不影响启动。 */
function readStoredWidths(profile: PaneProfile): PaneWidths | undefined {
  try {
    const raw = window.localStorage.getItem(profile.storageKey);
    if (!raw) return undefined;
    const parsed = JSON.parse(raw) as Partial<PaneWidths>;
    if (typeof parsed.sidebar !== "number" || typeof parsed.list !== "number") return undefined;
    return { sidebar: parsed.sidebar, list: parsed.list };
  } catch {
    return undefined;
  }
}

function persistWidths(profile: PaneProfile, widths: PaneWidths): void {
  try {
    window.localStorage.setItem(profile.storageKey, JSON.stringify(widths));
  } catch {
    // 存不下就算了，不阻塞使用。
  }
}

/** 窗口分给「左栏 + 中栏」的总宽度：先给内容区留够最小宽度。 */
export function paneBudget(
  windowWidth: number,
  readerMinWidth: number = READER_MIN_WIDTH,
): number {
  return windowWidth - RAIL_WIDTH - RESIZER_WIDTH * 2 - readerMinWidth;
}

/**
 * 把两栏宽度收进当前窗口可用空间。
 * 顺序：先保内容区最小宽度，再挤中栏，最后才挤左栏。
 */
export function fitPaneWidths(
  widths: PaneWidths,
  availableForPanes: number,
  profile: PaneProfile = INBOX_PANE_PROFILE,
): PaneWidths {
  const sidebarLimits = profile.sidebarLimits;
  const listLimits = profile.listLimits;

  let sidebar = clamp(Math.round(widths.sidebar), sidebarLimits.min, sidebarLimits.max);
  let list = clamp(Math.round(widths.list), listLimits.min, listLimits.max);

  const budget = Math.max(0, Math.round(availableForPanes));
  if (sidebar + list <= budget) return { sidebar, list };

  const minTotal = sidebarLimits.min + listLimits.min;
  if (budget <= minTotal) {
    // 极端窄屏：两栏按比例缩到最小宽度以下，优先保住内容区。
    const scale = budget / minTotal;
    return {
      sidebar: Math.max(0, Math.floor(sidebarLimits.min * scale)),
      list: Math.max(0, Math.floor(listLimits.min * scale)),
    };
  }

  const overflow = sidebar + list - budget;
  const listShrink = Math.min(overflow, list - listLimits.min);
  list -= listShrink;
  const rest = overflow - listShrink;
  if (rest > 0) sidebar -= Math.min(rest, sidebar - sidebarLimits.min);
  return { sidebar, list };
}

/**
 * 三栏的宽度状态（左栏 + 中栏，内容区占剩下的）：
 * - 拖动中只改内存，鼠标松手后才写本地存储；
 * - 键盘调整和双击复位这类低频操作直接落盘；
 * - 窗口变窄时自动收窄，保证内容区不小于给定的最小宽度。
 *
 * 收件箱与通讯录共用这一份逻辑，靠 `profile` 区分存储键与上下限。
 */
export function usePaneWidths(
  readerMinWidth: number = READER_MIN_WIDTH,
  profile: PaneProfile = INBOX_PANE_PROFILE,
) {
  const [stored, setStored] = useState<PaneWidths>(
    () => readStoredWidths(profile) ?? profile.defaults,
  );
  const storedRef = useRef(stored);
  const [windowWidth, setWindowWidth] = useState(() =>
    typeof window === "undefined" ? 1440 : window.innerWidth,
  );

  useEffect(() => {
    storedRef.current = stored;
  }, [stored]);

  useEffect(() => {
    const onResize = () => setWindowWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const widths = useMemo(
    () => fitPaneWidths(stored, paneBudget(windowWidth, readerMinWidth), profile),
    [stored, windowWidth, readerMinWidth, profile],
  );

  /** 拖动中只改内存，等松手再落盘，避免每次移动都写一次本地存储。 */
  const apply = useCallback((next: PaneWidths) => {
    storedRef.current = next;
    setStored(next);
  }, []);

  /** 低频操作直接落盘。 */
  const commit = useCallback(
    (next: PaneWidths) => {
      apply(next);
      persistWidths(profile, next);
    },
    [apply, profile],
  );

  /** 鼠标松手时把当前宽度写进本地存储。 */
  const persist = useCallback(() => {
    persistWidths(profile, storedRef.current);
  }, [profile]);

  const setSidebar = useCallback(
    (value: number) => {
      apply({
        sidebar: clamp(Math.round(value), profile.sidebarLimits.min, profile.sidebarLimits.max),
        list: widths.list,
      });
    },
    [apply, profile, widths.list],
  );

  const setList = useCallback(
    (value: number) => {
      apply({
        sidebar: widths.sidebar,
        list: clamp(Math.round(value), profile.listLimits.min, profile.listLimits.max),
      });
    },
    [apply, profile, widths.sidebar],
  );

  const reset = useCallback(() => commit(profile.defaults), [commit, profile]);

  return {
    widths,
    /** 当前档位的上下限，界面直接喂给 PaneResizer。 */
    limits: { sidebar: profile.sidebarLimits, list: profile.listLimits },
    setSidebar,
    setList,
    reset,
    persist,
  };
}