import { useCallback, useEffect, useRef, type CSSProperties, type JSX } from "react";

import AccountForm from "./AccountForm";

/** 打开「添加邮箱」弹窗的入口：邮箱栏或设置页。 */
export type AddAccountSource = "mailbox" | "settings";

export type AddAccountDialogProps = {
  open: boolean;
  source: AddAccountSource;
  proxiesVersion?: number;
  onClose: () => void;
  onSaved: (accountId: string) => void;
};

const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** 弹窗里所有能按到的元素；浏览器里优先取真正可见的，兜底再退回全部。 */
function focusableWithin(root: HTMLElement | null): HTMLElement[] {
  if (root === null) return [];
  const all = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
  const visible = all.filter((element) => element.offsetParent !== null);
  return visible.length > 0 ? visible : all;
}

/** 打开弹窗后焦点要落在第一个输入项，而不是标题或关闭按钮。 */
function firstField(root: HTMLElement | null): HTMLElement | null {
  if (root === null) return null;
  return root.querySelector<HTMLElement>(
    "input:not([disabled]), select:not([disabled]), textarea:not([disabled])",
  );
}

const BACKDROP_STYLE: CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(15, 23, 42, 0.45)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: 16,
  zIndex: 1000,
};

const DIALOG_STYLE: CSSProperties = {
  width: "min(760px, 90vw)",
  maxHeight: "90vh",
  display: "flex",
  flexDirection: "column",
  background: "var(--surface, #ffffff)",
  color: "inherit",
  borderRadius: 12,
  boxShadow: "0 24px 60px rgba(15, 23, 42, 0.35)",
  overflow: "hidden",
};

const HEADER_STYLE: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 12,
  padding: "16px 20px",
  borderBottom: "1px solid rgba(148, 163, 184, 0.35)",
};

const STEP_STYLE: CSSProperties = {
  margin: 0,
  padding: "12px 20px 0",
  fontSize: 13,
  lineHeight: 1.6,
  opacity: 0.85,
};

const BODY_STYLE: CSSProperties = {
  padding: "8px 20px 20px",
  overflowY: "auto",
};

export default function AddAccountDialog({
  open,
  source,
  proxiesVersion,
  onClose,
  onSaved,
}: AddAccountDialogProps): JSX.Element {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const dirtyRef = useRef(false);
  const busyRef = useRef(false);

  // 打开时记住是谁打开的、把焦点送进第一个输入框；关闭时把焦点还回去。
  useEffect(() => {
    if (!open) return;
    dirtyRef.current = false;
    busyRef.current = false;
    const previous = document.activeElement;
    restoreFocusRef.current = previous instanceof HTMLElement ? previous : null;
    const timer = window.setTimeout(() => {
      firstField(dialogRef.current)?.focus();
    }, 0);
    return () => {
      window.clearTimeout(timer);
      const target = restoreFocusRef.current;
      if (target !== null && document.contains(target)) target.focus();
    };
  }, [open]);

  const requestClose = useCallback(() => {
    // 正在测试 / 保存 / 授权时先别关，避免半截请求把状态搞乱。
    if (busyRef.current) return;
    if (dirtyRef.current && !window.confirm("表单里还有没保存的内容，确定放弃吗？")) {
      return;
    }
    onClose();
  }, [onClose]);

  // Esc 关闭；Tab 在弹窗内循环，别跑到后面的页面上去。
  useEffect(() => {
    if (!open) return;
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        requestClose();
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
  }, [open, requestClose]);

  // 关着的时候渲染一个空片段：既满足 JSX.Element 返回类型，也不会挂载表单。
  if (!open) return <></>;

  return (
    <div
      style={BACKDROP_STYLE}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="add-account-title"
        style={DIALOG_STYLE}
      >
        <header style={HEADER_STYLE}>
          <h2 id="add-account-title" style={{ margin: 0, fontSize: 18 }}>
            添加邮箱
          </h2>
          <button
            type="button"
            aria-label="关闭"
            onClick={requestClose}
            style={{
              border: "none",
              background: "transparent",
              fontSize: 22,
              lineHeight: 1,
              cursor: "pointer",
              color: "inherit",
            }}
          >
            ×
          </button>
        </header>

        <p style={STEP_STYLE}>
          操作步骤：先填邮箱地址，服务器参数会自动补齐；微软邮箱（Outlook / Hotmail / Live / MSN）
          和谷歌 Gmail 会自动改用 OAuth2，点「浏览器授权」登录即可；其他邮箱点「连接自检」确认能收能发，通过后保存。
          密码、授权码和令牌只进系统保险箱，不会写进数据库或日志。
        </p>

        <div style={BODY_STYLE}>
          <AccountForm
            account={null}
            source={source}
            proxiesVersion={proxiesVersion}
            onSaved={onSaved}
            onCancel={requestClose}
            onDirtyChange={(value) => {
              dirtyRef.current = value;
            }}
            onBusyChange={(value) => {
              busyRef.current = value;
            }}
          />
        </div>
      </div>
    </div>
  );
}