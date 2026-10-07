import { useCallback, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";

type ReaderResizerProps = {
  /** 中文可读名称，读屏会念出来。 */
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  /** 鼠标松手或键盘调整完成后调用，用于把高度落盘。 */
  onCommit: () => void;
  onReset: () => void;
};

/** 方向键每次调整的像素。 */
const STEP = 12;
/** 按住 Shift 时的调整步长。 */
const BIG_STEP = 36;

/**
 * 正文与附件之间的上下拖动条：
 * 鼠标上下拖动、方向键调整、Home 回默认高度、双击复位。
 * 拖动过程只改界面，松手时才通知外层落盘。
 */
export default function ReaderResizer({
  label,
  value,
  min,
  max,
  onChange,
  onCommit,
  onReset,
}: ReaderResizerProps) {
  const drag = useRef<{ startY: number; startValue: number } | null>(null);

  const handlePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      drag.current = { startY: event.clientY, startValue: value };
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    [value],
  );

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const state = drag.current;
      if (!state) return;
      // 分隔条向上拖表示附件变高，所以鼠标位移要反着算。
      onChange(state.startValue - (event.clientY - state.startY));
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
      if (event.key === "Home") {
        event.preventDefault();
        onReset();
        return;
      }
      let next: number | undefined;
      if (event.key === "ArrowUp") next = value + step;
      else if (event.key === "ArrowDown") next = value - step;
      else if (event.key === "End") next = max;
      if (next === undefined) return;
      event.preventDefault();
      onChange(next);
      onCommit();
    },
    [max, onChange, onCommit, onReset, value],
  );

  return (
    <div
      className="reader-resizer"
      role="separator"
      tabIndex={0}
      aria-orientation="horizontal"
      aria-label={label}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={max}
      title={`${label}：拖动或按上下方向键调整，Home 回默认高度，双击复位`}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerEnd}
      onPointerCancel={handlePointerEnd}
      onDoubleClick={onReset}
      onKeyDown={handleKeyDown}
    >
      <span className="reader-resizer-grip" aria-hidden="true" />
    </div>
  );
}