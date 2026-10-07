//! 截图叠加窗。
//!
//! 这个窗口只在截图时临时出现：铺满整块屏，背景是刚才拍下的整屏冻结图（软件自己不在里面），
//! 用户拖框选区域。松手时把选区的物理像素坐标交给后端裁剪、落盘，主窗口随后自动回来。
//! 按 Esc 取消。

import { useCallback, useEffect, useRef, useState } from "react";

import { api, describeError } from "./api";

interface Point {
  x: number;
  y: number;
}

interface Area {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 选区小于这个尺寸就当是误点，不触发截图。 */
const MIN_SIZE = 4;

/** 由起点与当前点算出矩形（允许往左、往上拉）。 */
export function areaFrom(start: Point | undefined, current: Point | undefined): Area | null {
  if (!start || !current) return null;
  const x = Math.min(start.x, current.x);
  const y = Math.min(start.y, current.y);
  return {
    x,
    y,
    width: Math.abs(current.x - start.x),
    height: Math.abs(current.y - start.y),
  };
}

/** 截图叠加窗。 */
export default function ScreenshotOverlay() {
  const [preview, setPreview] = useState("");
  const [start, setStart] = useState<Point>();
  const [current, setCurrent] = useState<Point>();
  const [error, setError] = useState("");
  const busyRef = useRef(false);

  /** 拿整屏冻结图当背景；拿不到就没法框选。 */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const dataUrl = await api.takeScreenshotPreview();
        if (!cancelled) setPreview(dataUrl);
      } catch (caught) {
        if (!cancelled) setError(describeError(caught));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const cancel = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    try {
      await api.cancelScreenshot();
    } catch {
      // 取消失败也不能把用户留在叠加窗里，后面还有 Esc 和后端兜底。
    }
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        void cancel();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [cancel]);

  const area = areaFrom(start, current);

  const handleDown = (event: React.MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0 || busyRef.current) return;
    const point = { x: event.clientX, y: event.clientY };
    setStart(point);
    setCurrent(point);
    setError("");
  };

  const handleMove = (event: React.MouseEvent<HTMLDivElement>) => {
    if (!start || busyRef.current) return;
    setCurrent({ x: event.clientX, y: event.clientY });
  };

  const handleUp = async () => {
    if (!area || busyRef.current) return;
    if (area.width < MIN_SIZE || area.height < MIN_SIZE) {
      setStart(undefined);
      setCurrent(undefined);
      return;
    }
    busyRef.current = true;
    try {
      const ratio = window.devicePixelRatio || 1;
      await api.finishScreenshot(
        Math.round(area.x * ratio),
        Math.round(area.y * ratio),
        Math.round(area.width * ratio),
        Math.round(area.height * ratio),
      );
    } catch (caught) {
      busyRef.current = false;
      setError(describeError(caught));
      setStart(undefined);
      setCurrent(undefined);
    }
  };

  return (
    <div
      className="screenshot-overlay"
      style={preview ? { backgroundImage: `url(${preview})` } : undefined}
      onMouseDown={handleDown}
      onMouseMove={handleMove}
      onMouseUp={() => void handleUp()}
    >
      {area ? (
        <>
          <div className="screenshot-mask" style={{ left: 0, top: 0, right: 0, height: area.y }} />
          <div
            className="screenshot-mask"
            style={{ left: 0, top: area.y, width: area.x, height: area.height }}
          />
          <div
            className="screenshot-mask"
            style={{ left: area.x + area.width, top: area.y, right: 0, height: area.height }}
          />
          <div
            className="screenshot-mask"
            style={{ left: 0, top: area.y + area.height, right: 0, bottom: 0 }}
          />
          <div
            className="screenshot-rect"
            style={{ left: area.x, top: area.y, width: area.width, height: area.height }}
          />
          <span className="screenshot-size" style={{ left: area.x, top: area.y - 24 }}>
            {Math.round(area.width)} × {Math.round(area.height)}
          </span>
        </>
      ) : (
        <div className="screenshot-mask" style={{ inset: 0 }} />
      )}
      <p className="screenshot-hint">拖动选择要插入的区域，按 Esc 取消</p>
      {error && <p className="screenshot-error">操作失败：{error}</p>}
    </div>
  );
}