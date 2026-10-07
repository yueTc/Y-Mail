//! 窗口级「把文件拖进来」订阅。
//!
//! Tauri 2 默认由原生层接管文件拖拽，网页侧收不到 HTML5 的 drop 事件，
//! 只能订阅 webview 的 drag-drop 事件；浏览器预览里没有这套机制，订阅直接返回空。
//! 这里只透传本地路径，不读文件内容、不联网。

import { getCurrentWebview } from "@tauri-apps/api/webview";

/** 一次拖拽事件；坐标是物理像素，相对 webview 左上角。 */
export interface WindowFileDropEvent {
  /** 进入 / 悬停 / 放下 / 离开。 */
  type: "enter" | "over" | "drop" | "leave";
  /** 正在拖或已放下的本地路径；悬停与离开时是空数组。 */
  paths: string[];
  /** 只有悬停与离开没有坐标。 */
  position?: { x: number; y: number };
}

/** 当前是不是跑在 Tauri 外壳里。 */
export function isTauriRuntime(): boolean {
  const internals = (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  return typeof internals === "object" && internals !== null;
}

/**
 * 订阅拖拽事件，返回退订函数。
 *
 * 拿不到订阅（浏览器预览、外壳异常）时不报错，只返回一个空退订函数：
 * 拖拽用不了还有「选择文件」和路径输入兜底。
 */
export function subscribeFileDrop(handler: (event: WindowFileDropEvent) => void): () => void {
  if (!isTauriRuntime()) return () => {};

  let stopped = false;
  let unlisten: (() => void) | undefined;
  void getCurrentWebview()
    .onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "leave") {
        handler({ type: "leave", paths: [] });
        return;
      }
      const position = { x: payload.position.x, y: payload.position.y };
      if (payload.type === "enter") {
        handler({ type: "enter", paths: payload.paths, position });
      } else if (payload.type === "over") {
        handler({ type: "over", paths: [], position });
      } else {
        handler({ type: "drop", paths: payload.paths, position });
      }
    })
    .then((stop) => {
      if (stopped) stop();
      else unlisten = stop;
    })
    .catch(() => {
      // 订阅失败只是拖拽不可用，不影响写信本身。
    });

  return () => {
    stopped = true;
    unlisten?.();
  };
}