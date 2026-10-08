import { useCallback, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";
import { t } from "./i18n";

type PaneResizerProps = {
  /** 中文可读名称，读屏会念出来。 */
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  /** 鼠标松手或键盘调整完成后调用，用于把宽度落盘。 */
  onCommit: () => void;
  onReset: () => void;
};

/** 方向键每次调整的像素。 */
const STEP = 16;
/** 按住 Shift 时的调整步长。 */
const BIG_STEP = 48;

/**
 * 两栏之间的拖动条：
 * 鼠标拖动、左右方向键调整、双击复位。
 * 鼠标拖动过程只改界面，松手时才通知外层落盘。
 */
export default function PaneResizer({
  label,
  value,
  min,
  max,
  onChange,
  onCommit,
  onReset,
}: PaneResizerProps) {
  const drag = useRef<{ startX: number; startValue: number } | null>(null);

  const handlePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      drag.current = { startX: event.clientX, startValue: value };
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    [value],
  );

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const state = drag.current;
      if (!state) return;
      onChange(state.startValue + (event.clientX - state.startX));
    },
    [onChange],
  );

  const handlePointerEnd = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (!drag.current) return;
      drag.current = null;
      onCommit();
      try {
        event.currentTarget.releasePointerCapture(event.pointerId);
      } catch {
        // 指针已经释放过，忽略。
      }
    },
    [onCommit],
  );

  const handleKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      const step = event.shiftKey ? BIG_STEP : STEP;
      let next: number | undefined;
      if (event.key === "ArrowLeft") next = value - step;
      else if (event.key === "ArrowRight") next = value + step;
      else if (event.key === "Home") next = min;
      else if (event.key === "End") next = max;
      if (next === undefined) return;
      event.preventDefault();
      onChange(next);
      onCommit();
    },
    [max, min, onChange, onCommit, value],
  );

  return (
    <div
      className="pane-resizer"
      role="separator"
      tabIndex={0}
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      title={t("{0}：拖动或按左右方向键调整，双击复位", [label])}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerEnd}
      onPointerCancel={handlePointerEnd}
      onDoubleClick={onReset}
      onKeyDown={handleKeyDown}
    >
      <span className="pane-resizer-grip" aria-hidden="true" />
    </div>
  );
}