//! AI 外发授权弹窗（Wave 7）。
//!
//! 真正发起模型调用前必须经过这里。弹窗只展示后端预览返回的
//! 域名、模型和是否本地，不展示邮件正文，也不写入任何存储。
//!
//! 无障碍：打开后焦点进弹窗、Tab 在弹窗内循环、Esc 关闭，关闭后焦点还给触发按钮。

import { useCallback, useEffect, useRef } from "react";

import type { AiAuthorization, AiFunction } from "./api";

const FUNCTION_LABEL: Record<AiFunction, string> = {
  translate: "翻译",
  summary: "摘要",
  polish: "润色",
  draft: "起草",
};

/** 弹窗里所有能按到的元素；浏览器里优先取真正可见的，兜底再退回全部。 */
const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusableWithin(root: HTMLElement | null): HTMLElement[] {
  if (root === null) return [];
  const all = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
  const visible = all.filter((element) => element.offsetParent !== null);
  return visible.length > 0 ? visible : all;
}

/**
 * 打开时焦点落在哪：优先标了 `data-autofocus` 的元素（这里是「取消」）。
 * 故意不默认落在「确认并调用」上——这是外发确认，手一抖不该直接把内容发出去。
 */
function initialFocus(root: HTMLElement | null): HTMLElement | null {
  if (root === null) return null;
  return root.querySelector<HTMLElement>("[data-autofocus]") ?? focusableWithin(root)[0] ?? null;
}

export interface AiAuthorizationDialogProps {
  preview: AiAuthorization;
  busy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

/** 通用的 AI 调用确认框。 */
export default function AiAuthorizationDialog({
  preview,
  busy = false,
  onCancel,
  onConfirm,
}: AiAuthorizationDialogProps) {
  const dialogRef = useRef<HTMLElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const busyRef = useRef(busy);

  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);

  // 弹窗是随用随挂的：挂上时记住是谁打开的、把焦点送进来；卸载时把焦点还回去。
  useEffect(() => {
    const previous = document.activeElement;
    restoreFocusRef.current = previous instanceof HTMLElement ? previous : null;
    const timer = window.setTimeout(() => {
      initialFocus(dialogRef.current)?.focus();
    }, 0);
    return () => {
      window.clearTimeout(timer);
      const target = restoreFocusRef.current;
      if (target !== null && document.contains(target)) target.focus();
    };
  }, []);

  const requestCancel = useCallback(() => {
    // 正在调用时先别关，避免半截请求把状态搞乱。
    if (busyRef.current) return;
    onCancel();
  }, [onCancel]);

  // Esc 关闭；Tab 在弹窗内循环，别跑到后面的页面上去。
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        requestCancel();
        return;
      }
      if (event.key !== "Tab") return;
      const root = dialogRef.current;
      if (root === null) return;
      const items = focusableWithin(root);
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      const inside = active instanceof HTMLElement && root.contains(active);
      if (event.shiftKey) {
        if (!inside || active === first) {
          event.preventDefault();
          last.focus();
        }
      } else if (!inside || active === last) {
        event.preventDefault();
        first.focus();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [requestCancel]);

  return (
    <div className="ai-modal-backdrop" role="presentation">
      <section
        ref={dialogRef}
        className="ai-modal"
        role="dialog"
        aria-modal="true"
        aria-label="确认 AI 外发"
        aria-busy={busy}
      >
        <h3>确认 AI 调用</h3>
        <p>
          这次要执行「{FUNCTION_LABEL[preview.function as AiFunction] ?? preview.function}」，
          请确认目标后再发送。
        </p>
        <dl className="ai-modal-targets">
          <dt>要发给哪个域名</dt>
          <dd>{preview.host}</dd>
          <dt>用哪个模型</dt>
          <dd>{preview.model}</dd>
          <dt>是不是本地服务</dt>
          <dd>{preview.local ? "是，本地服务，内容不离开这台电脑" : "不是，内容会发送到远程站点"}</dd>
          <dt>站点</dt>
          <dd>{preview.providerLabel}</dd>
        </dl>
        <p className="hint">
          确认后这次授权在 {preview.expiresInSeconds} 秒内有效，并且只能用一次。
        </p>
        <div className="form-actions">
          <button type="button" className="primary" onClick={onConfirm} disabled={busy}>
            {busy ? "正在调用……" : "确认并调用"}
          </button>
          <button type="button" data-autofocus onClick={requestCancel} disabled={busy}>
            取消
          </button>
        </div>
      </section>
    </div>
  );
}