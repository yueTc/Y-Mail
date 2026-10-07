import { useCallback, useEffect, useMemo, useRef, useState } from "react";

/** 附件区的默认高度。 */
export const DEFAULT_ATTACHMENT_HEIGHT = 220;
/** 附件区最小高度，保证标题和至少一条附件看得见。 */
export const MIN_ATTACHMENT_HEIGHT = 96;
/** 附件区最大高度，避免在超大窗口里把正文挤太小。 */
export const MAX_ATTACHMENT_HEIGHT = 520;
/** 正文区最小高度。 */
export const READER_BODY_MIN_HEIGHT = 180;
/** 上下拖动条占用的高度，和样式保持一致。 */
export const READER_RESIZER_HEIGHT = 8;

export const ATTACHMENT_HEIGHT_KEY = "ymail.reader-attachment-height.v1";

export type AttachmentHeightLimits = {
  min: number;
  max: number;
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * 按阅读区可用高度算附件区上下限。
 *
 * 正常空间下先给正文留够最小高度；极窄窗口里按比例收缩，
 * 但仍保证正文和附件两边都是正数，不会把一侧拖没。
 */
export function attachmentHeightLimits(availableHeight?: number): AttachmentHeightLimits {
  if (typeof availableHeight !== "number" || !Number.isFinite(availableHeight) || availableHeight <= 0) {
    return { min: MIN_ATTACHMENT_HEIGHT, max: MAX_ATTACHMENT_HEIGHT };
  }

  const available = Math.max(0, Math.floor(availableHeight));
  const usable = Math.max(1, available - READER_RESIZER_HEIGHT);
  const normalMax = Math.min(MAX_ATTACHMENT_HEIGHT, usable - READER_BODY_MIN_HEIGHT);
  if (normalMax >= MIN_ATTACHMENT_HEIGHT) {
    return { min: MIN_ATTACHMENT_HEIGHT, max: normalMax };
  }

  const bodyFloor = Math.max(1, Math.min(READER_BODY_MIN_HEIGHT, Math.floor(usable / 2)));
  const max = Math.max(1, usable - bodyFloor);
  return { min: Math.min(MIN_ATTACHMENT_HEIGHT, max), max };
}

/** 把任意高度校正到当前合法范围。 */
export function clampAttachmentHeight(value: number, limits: AttachmentHeightLimits): number {
  if (!Number.isFinite(value)) return clamp(DEFAULT_ATTACHMENT_HEIGHT, limits.min, limits.max);
  return clamp(Math.round(value), limits.min, limits.max);
}

/** 读本地高度记忆；数据坏了就回默认值。 */
function readStoredAttachmentHeight(): number | undefined {
  try {
    const raw = window.localStorage.getItem(ATTACHMENT_HEIGHT_KEY);
    if (!raw) return undefined;
    const parsed = Number(raw);
    return Number.isFinite(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function persistAttachmentHeight(height: number): void {
  try {
    window.localStorage.setItem(ATTACHMENT_HEIGHT_KEY, String(Math.round(height)));
  } catch {
    // 存不下就算了，不阻塞读信。
  }
}

/** 观察一个元素的高度，窗口缩放和布局变化时都会更新。 */
export function useElementHeight<T extends HTMLElement>() {
  const [element, setElement] = useState<T | null>(null);
  const [height, setHeight] = useState<number | undefined>();
  const ref = useCallback((node: T | null) => setElement(node), []);

  useEffect(() => {
    if (!element) {
      setHeight(undefined);
      return;
    }

    const update = () => {
      setHeight(element.clientHeight > 0 ? element.clientHeight : undefined);
    };
    update();

    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(update);
      observer.observe(element);
    }
    window.addEventListener("resize", update);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", update);
    };
  }, [element]);

  return { ref, height };
}

/**
 * 附件区高度状态：
 * - 拖动中只改内存，松手后写本地存储；
 * - 窗口变小后显示高度自动收进合法范围，但不覆盖用户记忆；
 * - Home / 双击回默认高度并落盘。
 */
export function useAttachmentHeight(availableHeight?: number) {
  const [stored, setStored] = useState<number>(() => readStoredAttachmentHeight() ?? DEFAULT_ATTACHMENT_HEIGHT);
  const storedRef = useRef(stored);

  useEffect(() => {
    storedRef.current = stored;
  }, [stored]);

  const limits = useMemo(() => attachmentHeightLimits(availableHeight), [availableHeight]);
  const height = clampAttachmentHeight(stored, limits);

  const apply = useCallback(
    (value: number) => {
      const next = clampAttachmentHeight(value, limits);
      storedRef.current = next;
      setStored(next);
      return next;
    },
    [limits],
  );

  const persist = useCallback(() => {
    persistAttachmentHeight(storedRef.current);
  }, []);

  const commit = useCallback(
    (value?: number) => {
      if (value !== undefined) apply(value);
      persist();
    },
    [apply, persist],
  );

  const reset = useCallback(() => {
    apply(DEFAULT_ATTACHMENT_HEIGHT);
    persist();
  }, [apply, persist]);

  return {
    height,
    min: limits.min,
    max: limits.max,
    setHeight: apply,
    commit,
    reset,
  };
}